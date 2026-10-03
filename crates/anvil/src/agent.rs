// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `pistack agent` loop.
//!
//! ```text
//! hello ──► [heartbeat task every N s] + [long-poll commands ──► execute ──► report]
//!   ▲                                                    │
//!   └──── transport error / server says "unknown" ◄──────┘  (backoff, re-hello)
//! ```
//!
//! The command poll and the heartbeat run as separate tasks on purpose: a
//! `select!` that dropped an in-flight poll to send a heartbeat would lose
//! any command the server had just marked delivered.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use pistack_rpc::proto::{
    AgentHello, Command, CommandKind, CommandResult, Heartbeat, MachineRef, MachineReport,
    MachineRequest, MAX_LONG_POLL_SECS,
};
use pistack_rpc::{AgentClient, RpcError};
use pistack_types::{ApiError, Phase, Resources};
use tokio_util::sync::CancellationToken;

use crate::facts::{self, HostFacts};
use crate::runtime::{select_runtimes, RunningMachine, Runtimes};

/// Environment variable that lets the mock runtime stand in for kinds with
/// no real runtime on this host.
pub const ALLOW_MOCK_ENV: &str = "PISTACK_ALLOW_MOCK";

/// Options for `pistack agent`.
pub struct AgentOpts {
    pub join: String,
    pub token: Option<String>,
    pub data_dir: std::path::PathBuf,
}

/// Path under the join URL where `pistack_rpc::router` is mounted by the
/// gateway. `--join http://server:8443` therefore talks to
/// `http://server:8443/rpc/agents/...`.
pub const RPC_PREFIX: &str = "/rpc";

/// Shortest and longest pause between reconnect attempts.
const BACKOFF_MIN: Duration = Duration::from_secs(1);
const BACKOFF_MAX: Duration = Duration::from_secs(30);

/// Entry point for the host agent: gather facts, pick runtimes, run until
/// ctrl-c.
pub async fn run_agent(opts: AgentOpts) -> anyhow::Result<()> {
    let facts = facts::gather_at(&opts.data_dir);
    let allow_mock = mock_allowed(&facts.capabilities);
    let runtimes = select_runtimes(&facts.capabilities, allow_mock);
    if runtimes.is_empty() {
        anyhow::bail!(
            "anvil agent: no machine runtime available (no kvm/libvirt/youki detected; set {ALLOW_MOCK_ENV}=1 to run the mock)"
        );
    }
    tracing::info!(
        join = %opts.join,
        node = %facts.node_name,
        vcpus = facts.resources.vcpus,
        memory_mib = facts.resources.memory_mib,
        disk_gib = facts.resources.disk_gib,
        capabilities = ?facts.capabilities,
        runtimes = ?runtimes,
        mock = allow_mock,
        has_token = opts.token.is_some(),
        "anvil agent starting"
    );

    let base = join_url(&opts.join);
    let mut client = AgentClient::new(base, facts.node_name.clone())?;
    if let Some(t) = opts.token {
        client = client.with_token(t);
    }

    let shutdown = CancellationToken::new();
    let sd = shutdown.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            tracing::info!("anvil agent: shutting down");
            sd.cancel();
        }
    });

    run_agent_with_facts(client, runtimes, facts, shutdown).await
}

/// The mock is allowed when the operator asks for it, or when the host has
/// nothing real to offer (a laptop, CI). Logged either way so a production
/// host that silently fell back to the mock is visible.
fn mock_allowed(capabilities: &[String]) -> bool {
    let forced = std::env::var(ALLOW_MOCK_ENV)
        .map(|v| matches!(v.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false);
    let real = capabilities
        .iter()
        .any(|c| matches!(c.as_str(), "kvm" | "libvirt" | "youki"));
    if forced {
        tracing::info!("mock runtime enabled by {ALLOW_MOCK_ENV}");
        true
    } else if !real {
        tracing::warn!("no kvm/libvirt/youki on this host; using the mock runtime");
        true
    } else {
        false
    }
}

/// Append [`RPC_PREFIX`] unless the operator already pointed at it.
fn join_url(join: &str) -> String {
    let j = join.trim_end_matches('/');
    if j.ends_with(RPC_PREFIX) {
        j.to_owned()
    } else {
        format!("{j}{RPC_PREFIX}")
    }
}

/// Run the agent loop against `client` with `runtimes`, until `shutdown`.
/// Facts are gathered from this process; tests use this to drive a mock
/// runtime against an in-process control plane.
pub async fn run_agent_with(
    client: AgentClient,
    runtimes: Runtimes,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let mut facts = facts::gather();
    facts.node_name = client.node_name().to_owned();
    run_agent_with_facts(client, runtimes, facts, shutdown).await
}

/// Like [`run_agent_with`] but with the facts the agent should advertise,
/// so a test can pin a known node name and capacity.
pub async fn run_agent_with_facts(
    client: AgentClient,
    runtimes: Runtimes,
    facts: HostFacts,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let agent = Agent {
        client,
        runtimes,
        facts,
    };
    let mut backoff = BACKOFF_MIN;
    loop {
        if shutdown.is_cancelled() {
            return Ok(());
        }
        match agent.session(&shutdown).await {
            SessionEnd::Shutdown => return Ok(()),
            SessionEnd::Rehello => {
                // The server forgot us (restart): say hello again at once.
                backoff = BACKOFF_MIN;
            }
            SessionEnd::Error(e) => {
                tracing::warn!(error = %e, retry_in = ?backoff, "anvil agent: control plane unreachable");
                tokio::select! {
                    _ = shutdown.cancelled() => return Ok(()),
                    _ = tokio::time::sleep(backoff) => {}
                }
                backoff = (backoff * 2).min(BACKOFF_MAX);
            }
        }
    }
}

enum SessionEnd {
    Shutdown,
    Rehello,
    Error(RpcError),
}

struct Agent {
    client: AgentClient,
    runtimes: Runtimes,
    facts: HostFacts,
}

impl Agent {
    fn hello(&self) -> AgentHello {
        let mut capabilities = self.facts.capabilities.clone();
        for n in self.runtimes.names() {
            if !capabilities.contains(&n) {
                capabilities.push(n);
            }
        }
        AgentHello {
            node_name: self.facts.node_name.clone(),
            capabilities,
            resources: self.facts.resources,
            version: env!("CARGO_PKG_VERSION").to_owned(),
            labels: Default::default(),
        }
    }

    /// One hello followed by heartbeats and polls until something breaks.
    async fn session(&self, shutdown: &CancellationToken) -> SessionEnd {
        let ack = match self.client.hello(&self.hello()).await {
            Ok(a) => a,
            Err(e) => return SessionEnd::Error(e),
        };
        let interval = Duration::from_secs(ack.heartbeat_interval_secs.max(1));
        tracing::info!(
            node = %ack.node_name,
            heartbeat_secs = interval.as_secs(),
            "anvil agent: registered with control plane"
        );

        let session = shutdown.child_token();
        let forgotten = Arc::new(AtomicBool::new(false));
        let hb = tokio::spawn(self.clone_for_heartbeat().heartbeats(
            interval,
            session.clone(),
            forgotten.clone(),
        ));

        let wait =
            Duration::from_secs(MAX_LONG_POLL_SECS).min(interval.max(Duration::from_secs(1)));
        let end = loop {
            if forgotten.load(Ordering::Relaxed) {
                break SessionEnd::Rehello;
            }
            let cmds = tokio::select! {
                _ = session.cancelled() => break SessionEnd::Shutdown,
                r = self.client.next_commands(wait) => r,
            };
            match cmds {
                Ok(cmds) if cmds.is_empty() => {}
                Ok(cmds) => self.execute_all(cmds).await,
                Err(e) => break SessionEnd::Error(e),
            }
        };
        session.cancel();
        let _ = hb.await;
        end
    }

    fn clone_for_heartbeat(&self) -> HeartbeatTask {
        HeartbeatTask {
            client: self.client.clone(),
            runtimes: self.runtimes.clone(),
            resources: self.facts.resources,
        }
    }

    async fn execute_all(&self, cmds: Vec<Command>) {
        let mut results = Vec::with_capacity(cmds.len());
        for c in cmds {
            let id = c.id;
            let kind = c.kind;
            let r = match self.execute(c).await {
                Ok(payload) => CommandResult::ok(id).with_payload(payload),
                Err(e) => {
                    tracing::warn!(%id, %kind, error = %e, "anvil: command failed");
                    CommandResult::from_api_error(id, &e)
                }
            };
            results.push(r);
        }
        match self.client.report(&results).await {
            Ok(ack) if !ack.unknown.is_empty() => {
                tracing::warn!(unknown = ?ack.unknown, "anvil: server no longer knew some commands")
            }
            Ok(_) => {}
            // The server will time the commands out; the reconciler retries.
            Err(e) => tracing::warn!(error = %e, "anvil: reporting results failed"),
        }
    }

    async fn execute(&self, c: Command) -> Result<serde_json::Value, ApiError> {
        match c.kind {
            CommandKind::StartMachine => {
                let req: MachineRequest = payload(c.payload)?;
                tracing::info!(machine = %req.id, name = %req.name, kind = ?req.kind, "anvil: start");
                let rt = self.runtimes.start(&req).await?;
                serde_json::to_value(rt).map_err(|e| ApiError::internal(e.to_string()))
            }
            CommandKind::StopMachine => {
                let r: MachineRef = payload(c.payload)?;
                tracing::info!(machine = %r.id, "anvil: stop");
                self.runtimes.stop(&r.id).await?;
                Ok(serde_json::Value::Null)
            }
            CommandKind::DeleteMachine => {
                let r: MachineRef = payload(c.payload)?;
                tracing::info!(machine = %r.id, "anvil: delete");
                self.runtimes.delete(&r.id).await?;
                Ok(serde_json::Value::Null)
            }
            other => Err(ApiError::not_implemented(format!(
                "anvil: {other} is not implemented yet"
            ))),
        }
    }
}

fn payload<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, ApiError> {
    serde_json::from_value(v)
        .map_err(|e| ApiError::bad_request(format!("bad command payload: {e}")))
}

struct HeartbeatTask {
    client: AgentClient,
    runtimes: Runtimes,
    resources: Resources,
}

impl HeartbeatTask {
    async fn heartbeats(
        self,
        interval: Duration,
        stop: CancellationToken,
        forgotten: Arc<AtomicBool>,
    ) {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        // The first tick fires immediately; a heartbeat right after hello
        // delivers the machine list without waiting a full interval.
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tick.tick() => {}
            }
            let machines = self.runtimes.list().await;
            let hb = Heartbeat {
                node_name: self.client.node_name().to_owned(),
                resources_free: self.resources,
                machines: machines.into_iter().map(report).collect(),
                at: Utc::now(),
            };
            match self.client.heartbeat(&hb).await {
                Ok(ack) if !ack.known => {
                    tracing::info!("anvil: control plane does not know this node; re-registering");
                    forgotten.store(true, Ordering::Relaxed);
                    return;
                }
                Ok(ack) => {
                    tracing::debug!(waiting = ack.commands_waiting, "anvil: heartbeat");
                }
                Err(e) => tracing::warn!(error = %e, "anvil: heartbeat failed"),
            }
        }
    }
}

/// How the agent describes a machine it holds. The phase is `Ready` for a
/// machine the runtime knows about regardless of power; `power` says more.
pub fn report(m: RunningMachine) -> MachineReport {
    MachineReport {
        id: m.id,
        phase: Phase::Ready,
        message: String::new(),
        resources: None,
        addresses: m.addresses,
        power: Some(m.power),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn join_url_adds_rpc_prefix_once() {
        assert_eq!(join_url("http://a:1"), "http://a:1/rpc");
        assert_eq!(join_url("http://a:1/"), "http://a:1/rpc");
        assert_eq!(join_url("http://a:1/rpc"), "http://a:1/rpc");
        assert_eq!(join_url("http://a:1/rpc/"), "http://a:1/rpc");
    }

    #[tokio::test]
    async fn run_agent_stops_on_shutdown_while_unreachable() {
        let client = AgentClient::new("http://127.0.0.1:1", "n1").unwrap();
        let shutdown = CancellationToken::new();
        let sd = shutdown.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(200)).await;
            sd.cancel();
        });
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::new());
        tokio::time::timeout(
            Duration::from_secs(10),
            run_agent_with(client, Runtimes::all(rt), shutdown),
        )
        .await
        .expect("agent returned")
        .expect("ok");
    }
}
