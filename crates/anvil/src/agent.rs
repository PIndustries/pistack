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
    MachineRequest, MigrateRequest, VolumeAttach, VolumeAttached, VolumeDetach, MAX_LONG_POLL_SECS,
};
use pistack_rpc::{AgentClient, RpcError};
use pistack_types::{ApiError, Phase, Resources};
use tokio_util::sync::CancellationToken;

use crate::facts::{self, HostFacts};
use crate::runtime::{select_runtimes, RunningMachine, Runtimes};

/// Environment variable that lets the mock runtime stand in for kinds with
/// no real runtime on this host.
pub const ALLOW_MOCK_ENV: &str = "PISTACK_ALLOW_MOCK";

/// When set, this IPv4 is the host's underlay address. An invalid value
/// disables the route lookup rather than falling through to it.
pub const UNDERLAY_IP_ENV: &str = "PISTACK_UNDERLAY_IP";

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
    let has_token = normalize_join_token(opts.token.clone()).is_some();
    tracing::info!(
        join = %opts.join,
        node = %facts.node_name,
        vcpus = facts.resources.vcpus,
        memory_mib = facts.resources.memory_mib,
        disk_gib = facts.resources.disk_gib,
        capabilities = ?facts.capabilities,
        runtimes = ?runtimes,
        mock = allow_mock,
        has_token,
        "anvil agent starting"
    );

    let client = connect(&opts.join, &facts.node_name, opts.token)?;

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

/// Blank and missing tokens stay unset, so `x-pistack-join` is not sent.
fn normalize_join_token(token: Option<String>) -> Option<String> {
    let t = token?.trim().to_owned();
    if t.is_empty() {
        None
    } else {
        Some(t)
    }
}

/// Build the agent client. The join token is attached only when it is non-empty.
fn connect(join: &str, node: &str, token: Option<String>) -> Result<AgentClient, RpcError> {
    let mut client = AgentClient::new(join_url(join), node)?;
    if let Some(t) = normalize_join_token(token) {
        client = client.with_token(t);
    }
    Ok(client)
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
            underlay_ip: discover_underlay(self.client.base_url()),
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
            CommandKind::AttachVolume => {
                let req: VolumeAttach = payload(c.payload)?;
                tracing::info!(machine = %req.machine, volume = %req.volume, "anvil: attach volume");
                let device = self.runtimes.attach_volume(&req).await?;
                serde_json::to_value(VolumeAttached { device })
                    .map_err(|e| ApiError::internal(e.to_string()))
            }
            CommandKind::DetachVolume => {
                let req: VolumeDetach = payload(c.payload)?;
                tracing::info!(machine = %req.machine, volume = %req.volume, "anvil: detach volume");
                self.runtimes.detach_volume(&req).await?;
                Ok(serde_json::Value::Null)
            }
            CommandKind::SyncOverlay => {
                tracing::info!("anvil: sync overlay");
                crate::overlay::apply_plan(&c.payload).await?;
                Ok(serde_json::Value::Null)
            }
            CommandKind::MigrateMachine => {
                let req: MigrateRequest = payload(c.payload)?;
                tracing::info!(machine = %req.id, dest = %req.dest, "anvil: migrate");
                let domain = crate::runtime::libvirt::domain_name(&req.id)?;
                crate::runtime::libvirt::migrate(&domain, &req.dest).await?;
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

/// Underlay address for a hello. `env_value` wins when it is `Some`, including
/// when that value is not a usable address. Otherwise the `src` of an
/// `ip -4 route get` line is used when that address is a private unicast.
pub fn underlay_from(env_value: Option<&str>, route_text: &str) -> Option<String> {
    if let Some(raw) = env_value {
        return parse_underlay(raw);
    }
    let mut parts = route_text.split_whitespace();
    while let Some(tok) = parts.next() {
        if tok == "src" {
            return parts.next().and_then(parse_underlay);
        }
    }
    None
}

fn parse_underlay(raw: &str) -> Option<String> {
    let ip: std::net::Ipv4Addr = raw.trim().parse().ok()?;
    underlay_ip_ok(ip).then(|| ip.to_string())
}

/// IPv4 unicast in 10/8, 172.16/12, 192.168/16, or 100.64/10.
fn underlay_ip_ok(ip: std::net::Ipv4Addr) -> bool {
    if ip.is_unspecified() || ip.is_broadcast() || ip.is_multicast() || ip.is_loopback() {
        return false;
    }
    let [a, b, _, _] = ip.octets();
    a == 10
        || (a == 172 && (16..=31).contains(&b))
        || (a == 192 && b == 168)
        || (a == 100 && (64..=127).contains(&b))
}

fn discover_underlay(api_base: &str) -> Option<String> {
    let found = match std::env::var_os(UNDERLAY_IP_ENV) {
        Some(value) => underlay_from(Some(&value.to_string_lossy()), ""),
        None => underlay_from(None, &underlay_route_text(api_base)),
    };
    tracing::debug!(underlay = ?found, "anvil: underlay address");
    found
}

fn underlay_route_text(api_base: &str) -> String {
    let Some(addr) = private_api_v4(api_base) else {
        return String::new();
    };
    let Ok(out) = std::process::Command::new("ip")
        .args(["-4", "route", "get", &addr])
        .output()
    else {
        return String::new();
    };
    if !out.status.success() {
        return String::new();
    }
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Host of `api_base` when it is a usable underlay IPv4. A hostname, a
/// public address, or loopback skips the route lookup.
fn private_api_v4(base: &str) -> Option<String> {
    let host = url_host(base)?;
    let ip: std::net::Ipv4Addr = host.parse().ok()?;
    underlay_ip_ok(ip).then(|| ip.to_string())
}

fn url_host(base: &str) -> Option<&str> {
    let rest = match base.split_once("://") {
        Some((_, rest)) => rest,
        None => base,
    };
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    if hostport.is_empty() {
        return None;
    }
    if let Some(v6) = hostport.strip_prefix('[') {
        return v6.split(']').next().filter(|s| !s.is_empty());
    }
    hostport.split(':').next().filter(|s| !s.is_empty())
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

    #[test]
    fn underlay_from_env_and_route() {
        assert_eq!(
            underlay_from(Some("10.66.66.5"), ""),
            Some("10.66.66.5".into())
        );
        assert_eq!(underlay_from(Some("8.8.8.8"), "src 10.0.0.1"), None);
        assert_eq!(
            underlay_from(None, "10.1.0.1 dev wg src 10.1.0.9 uid 1"),
            Some("10.1.0.9".into())
        );
        assert_eq!(
            underlay_from(None, "1.1.1.1 dev eth0 src 203.0.113.4"),
            None
        );
    }

    #[test]
    fn underlay_route_skips_non_private_api_hosts() {
        assert_eq!(
            private_api_v4("http://10.66.66.1:8443/rpc").as_deref(),
            Some("10.66.66.1")
        );
        assert_eq!(private_api_v4("http://8.8.8.8:8443/rpc"), None);
        assert_eq!(private_api_v4("http://127.0.0.1:1"), None);
        assert_eq!(private_api_v4("http://api.example:8443/rpc"), None);
        assert_eq!(private_api_v4("http://[2001:db8::1]:8443/rpc"), None);
    }

    #[test]
    fn blank_join_token_is_absent() {
        assert_eq!(normalize_join_token(None), None);
        assert_eq!(normalize_join_token(Some(String::new())), None);
        assert_eq!(normalize_join_token(Some("  ".into())), None);
        assert_eq!(
            normalize_join_token(Some(" pij_a ".into())),
            Some("pij_a".into())
        );
    }

    #[tokio::test]
    async fn agent_sends_join_header_only_when_token_set() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let record = seen.clone();
        let app = axum::Router::new().fallback(move |req: axum::extract::Request| {
            let record = record.clone();
            async move {
                let join = req
                    .headers()
                    .get(pistack_rpc::JOIN_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .map(str::to_owned);
                let auth = req
                    .headers()
                    .contains_key(axum::http::header::AUTHORIZATION);
                record
                    .lock()
                    .expect("lock")
                    .push((req.uri().path().to_owned(), join, auth));
                axum::http::StatusCode::NO_CONTENT
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let join = format!("http://{addr}");

        for token in [None, Some(String::new()), Some("   ".into())] {
            let client = connect(&join, "n1", token).unwrap();
            let _ = client
                .hello(&AgentHello {
                    node_name: "n1".into(),
                    capabilities: vec![],
                    resources: Resources::default(),
                    version: String::new(),
                    labels: Default::default(),
                    underlay_ip: None,
                })
                .await;
        }
        let client = connect(&join, "n1", Some(" pij_lab ".into())).unwrap();
        let hello = AgentHello {
            node_name: "n1".into(),
            capabilities: vec![],
            resources: Resources::default(),
            version: String::new(),
            labels: Default::default(),
            underlay_ip: None,
        };
        let _ = client.hello(&hello).await;
        let _ = client
            .heartbeat(&Heartbeat {
                node_name: "n1".into(),
                resources_free: Resources::default(),
                machines: vec![],
                at: Utc::now(),
            })
            .await;

        let got = seen.lock().expect("lock").clone();
        assert_eq!(got.len(), 5, "{got:?}");
        for row in &got[..3] {
            assert_eq!(row.1, None, "{row:?}");
            assert!(!row.2, "{row:?}");
            assert!(row.0.ends_with("/agents/hello"), "{row:?}");
        }
        assert_eq!(got[3].1.as_deref(), Some("pij_lab"));
        assert_eq!(got[4].1.as_deref(), Some("pij_lab"));
        assert!(got[3].0.ends_with("/agents/hello"));
        assert!(got[4].0.ends_with("/agents/heartbeat"));
        assert!(!got[3].2 && !got[4].2);
        assert!(!format!("{client:?}").contains("pij_lab"));
        server.abort();
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
