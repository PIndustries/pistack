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
//! A command marked `remember` runs once per id. Its result is kept (see
//! [`crate::done`]), a second delivery of the id is answered from what was
//! kept, and a result the server could not be told is sent again, to
//! whichever server the agent reaches next.
//!
//! The command poll and the heartbeat run as separate tasks on purpose: a
//! `select!` that dropped an in-flight poll to send a heartbeat would lose
//! any command the server had just marked delivered. Liveness and readiness
//! probes run on their own tick and ride back on the heartbeat. An exec
//! probe runs its argv through the runtime's command probe, which is youki
//! exec on a youki host.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use crate::images::ImageCache;
use chrono::Utc;
use keep::Pki;
use pistack_rpc::proto::{
    AgentHello, Command, CommandKind, CommandResult, DiskTarget, ExecRequest, Heartbeat, LogText,
    MachineKind, MachineRef, MachineReport, MachineRequest, MigrateRequest, PortPlug, VolumeAttach,
    VolumeAttached, VolumeDetach, MAX_LONG_POLL_SECS,
};
use pistack_rpc::{AgentClient, RpcError, RPC_INSECURE_ENV};
use pistack_types::{ApiError, ApiResult, Id, Phase, Resources};
use tokio_util::sync::CancellationToken;

use pistack_types::node::{self, NodeIdentity, NODE_NAME_ENV};

use crate::done::Completed;
use crate::facts::{self, HostFacts};
use crate::ovnhost::OvnHost;
use crate::ovnmeta::MetaNets;
use crate::probe::{self, ProbeBook, ProbeTarget};
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
    // Pod mounts live under this directory. The binary is the only caller.
    if std::env::var_os("PISTACK_DATA_DIR").is_none() {
        std::env::set_var("PISTACK_DATA_DIR", &opts.data_dir);
    }
    let mut facts = facts::gather_at(&opts.data_dir);
    let identity = agent_identity(&opts.data_dir, std::env::var(NODE_NAME_ENV).ok().as_deref())?;
    facts.node_name = identity.name;
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

    let client = connect_and_enroll(&opts, &facts.node_name).await?;

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

/// The node name this agent runs under, resolved once and kept in
/// `<data_dir>/node.name` (see [`pistack_types::node`]). An agent that
/// enrolled before the file existed keeps the name on its certificate, so
/// upgrading never renames a host.
pub fn agent_identity(
    data_dir: &std::path::Path,
    env: Option<&str>,
) -> anyhow::Result<NodeIdentity> {
    let enrolled = load_agent_pki(&data_dir.join("agent"))
        .ok()
        .flatten()
        .and_then(|pki| cert_node_name(&pki.cert_pem));
    let id =
        node::resolve_node_name_with(data_dir, env, enrolled.as_deref(), node::system_hostname)?;
    if let Some(old) = &id.replaced {
        tracing::warn!(
            node = %id.name,
            previous = %old,
            "{NODE_NAME_ENV} renamed this node; the control plane will see a new host"
        );
    }
    tracing::info!(node = %id.name, source = %id.source, "anvil agent: node identity");
    Ok(id)
}

/// Common name of the leaf certificate in `cert_pem`: the node name the
/// control plane enrolled it under.
pub fn cert_node_name(cert_pem: &str) -> Option<String> {
    let certs = keep::tls::certs_from_pem(cert_pem).ok()?;
    let leaf = certs.first()?;
    keep::ca::common_name_from_der(leaf).ok()
}

/// Build the agent client. Prefer a persisted client certificate issued for
/// `node`; otherwise enroll with the join token when present. A certificate
/// for another node is replaced by re-enrolling, never used, because the
/// control plane only accepts a certificate for its own node. Plain HTTP
/// without a certificate only works when the control plane sets
/// `PISTACK_RPC_INSECURE=1`.
async fn connect_and_enroll(opts: &AgentOpts, node: &str) -> anyhow::Result<AgentClient> {
    let base = join_url(&opts.join);
    let token = normalize_join_token(opts.token.clone());
    let cert_dir = opts.data_dir.join("agent");

    if let Some(pki) = load_agent_pki(&cert_dir)? {
        match persisted_cert_use(
            cert_node_name(&pki.cert_pem).as_deref(),
            node,
            token.is_some(),
        ) {
            CertUse::Keep => {
                tracing::info!(node, "using persisted agent client certificate");
                let mut client = AgentClient::with_mtls(&base, node, &pki.ca_pem, &pki)?;
                if let Some(t) = token {
                    client = client.with_token(t);
                }
                return Ok(client);
            }
            CertUse::Reenroll(cert_node) => {
                tracing::warn!(
                    node,
                    cert_node = cert_node.as_deref().unwrap_or("?"),
                    "persisted agent certificate is for another node; re-enrolling"
                );
            }
            CertUse::Refuse(cert_node) => {
                let cert_node = cert_node.as_deref().unwrap_or("an unreadable name");
                anyhow::bail!(
                    "agent certificate in {} is for node {cert_node}, but this node is {node}; \
                     pass a join token to re-enroll as {node}, or set {NODE_NAME_ENV}={cert_node}",
                    cert_dir.display()
                );
            }
        }
    }

    if let Some(t) = token.clone() {
        let bootstrap = AgentClient::new(&base, node)?.with_token(t);
        match bootstrap.enroll(node).await {
            Ok(enrolled) => {
                let pki = Pki {
                    cert_pem: enrolled.cert_pem,
                    key_pem: enrolled.key_pem,
                    ca_pem: enrolled.ca_pem,
                };
                save_agent_pki(&cert_dir, &pki)?;
                tracing::info!("enrolled agent client certificate");
                // Keep talking to whichever server enrolled us (the leader,
                // if the join address pointed at a follower).
                let client = AgentClient::with_mtls(bootstrap.base_url(), node, &pki.ca_pem, &pki)?;
                client.learn_servers(bootstrap.known_servers());
                return Ok(client);
            }
            Err(err) => {
                tracing::warn!(
                    error = %err,
                    "agent enroll failed; continuing without a client certificate"
                );
            }
        }
    } else if !rpc_insecure() {
        tracing::warn!(
            "no agent certificate and no join token; /rpc will reject this agent unless {RPC_INSECURE_ENV}=1"
        );
    }

    Ok(connect(&opts.join, node, opts.token.clone())?)
}

#[derive(Debug, PartialEq, Eq)]
enum CertUse {
    Keep,
    Reenroll(Option<String>),
    Refuse(Option<String>),
}

/// What to do with a persisted certificate whose common name is `cert_node`
/// when this agent runs as `node`.
fn persisted_cert_use(cert_node: Option<&str>, node: &str, have_token: bool) -> CertUse {
    match cert_node {
        Some(cn) if cn == node => CertUse::Keep,
        other if have_token => CertUse::Reenroll(other.map(str::to_owned)),
        other => CertUse::Refuse(other.map(str::to_owned)),
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

fn rpc_insecure() -> bool {
    std::env::var(RPC_INSECURE_ENV)
        .ok()
        .as_deref()
        .is_some_and(|v| v.trim() == "1")
}

fn load_agent_pki(dir: &std::path::Path) -> anyhow::Result<Option<Pki>> {
    let cert = dir.join("cert.pem");
    let key = dir.join("key.pem");
    let ca = dir.join("ca.pem");
    if !(non_empty(&cert) && non_empty(&key) && non_empty(&ca)) {
        return Ok(None);
    }
    Ok(Some(Pki {
        cert_pem: std::fs::read_to_string(&cert)?,
        key_pem: std::fs::read_to_string(&key)?,
        ca_pem: std::fs::read_to_string(&ca)?,
    }))
}

fn save_agent_pki(dir: &std::path::Path, pki: &Pki) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir)?;
    std::fs::write(dir.join("cert.pem"), &pki.cert_pem)?;
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let path = dir.join("key.pem");
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&path)?;
        file.write_all(pki.key_pem.as_bytes())?;
    }
    std::fs::write(dir.join("ca.pem"), &pki.ca_pem)?;
    Ok(())
}

fn non_empty(path: &std::path::Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
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
    let ovn = Arc::new(OvnHost::system());
    let meta = Arc::new(MetaNets::system());
    run_agent_on(client, runtimes, facts, ovn, meta, None, shutdown).await
}

async fn run_agent_on(
    client: AgentClient,
    runtimes: Runtimes,
    facts: HostFacts,
    ovn: Arc<OvnHost>,
    meta: Arc<MetaNets>,
    underlay: Option<String>,
    shutdown: CancellationToken,
) -> anyhow::Result<()> {
    let ovn_watch = Arc::new(AtomicBool::new(
        facts.capabilities.iter().any(|c| c == "ovs"),
    ));
    let agent = Agent {
        client,
        runtimes,
        facts,
        probes: Arc::new(ProbeBook::new()),
        images: ImageCache::new(&data_dir()),
        ovn,
        ovn_watch,
        meta,
        underlay,
        done: Arc::new(Completed::open(data_dir().join(crate::done::FILE))),
        unsent: Mutex::new(Vec::new()),
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
    probes: Arc<ProbeBook>,
    images: ImageCache,
    ovn: Arc<OvnHost>,
    /// Report OVN in heartbeats. Turned on by a hello answer that carries
    /// OVN settings, and from the start on a host that has Open vSwitch.
    ovn_watch: Arc<AtomicBool>,
    /// Metadata namespaces for guests on OVN networks.
    meta: Arc<MetaNets>,
    /// Underlay address to announce instead of looking one up.
    underlay: Option<String>,
    /// Results of remembered commands, by id.
    done: Arc<Completed>,
    /// Remembered results no server has taken yet.
    unsent: Mutex<Vec<CommandResult>>,
}

/// The agent's data directory: `PISTACK_DATA_DIR`, which `pistack agent`
/// sets from its flag, or the packaged default.
fn data_dir() -> std::path::PathBuf {
    std::env::var_os("PISTACK_DATA_DIR")
        .filter(|value| !value.is_empty())
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("/var/lib/pistack"))
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
            version: pistack_types::BUILD.to_owned(),
            labels: Default::default(),
            underlay_ip: self
                .underlay
                .clone()
                .or_else(|| discover_underlay(&self.client.base_url())),
        }
    }

    /// One hello followed by heartbeats and polls until something breaks.
    async fn session(&self, shutdown: &CancellationToken) -> SessionEnd {
        let hello = self.hello();
        let ack = match self.client.hello(&hello).await {
            Ok(a) => a,
            Err(e) => return SessionEnd::Error(e),
        };
        if let Some(cfg) = &ack.ovn {
            self.ovn_watch.store(true, Ordering::Relaxed);
            let mappings = crate::ovnhost::bridge_mappings_from_env();
            if let Err(e) = self
                .ovn
                .configure(
                    cfg,
                    hello.underlay_ip.as_deref(),
                    &self.facts.node_name,
                    mappings.as_deref(),
                )
                .await
            {
                tracing::warn!(error = %e.message, "anvil: this host is not set up for OVN");
            }
        }
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
        let probes = tokio::spawn(self.clone_for_probes().run(session.clone()));

        let wait =
            Duration::from_secs(MAX_LONG_POLL_SECS).min(interval.max(Duration::from_secs(1)));
        let end = loop {
            if forgotten.load(Ordering::Relaxed) {
                break SessionEnd::Rehello;
            }
            self.resend().await;
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
        let _ = probes.await;
        end
    }

    fn clone_for_heartbeat(&self) -> HeartbeatTask {
        HeartbeatTask {
            client: self.client.clone(),
            runtimes: self.runtimes.clone(),
            resources: self.facts.resources,
            data_dir: data_dir(),
            probes: self.probes.clone(),
            ovn: self.ovn.clone(),
            ovn_watch: self.ovn_watch.clone(),
            meta: self.meta.clone(),
        }
    }

    fn clone_for_probes(&self) -> ProbeLoop {
        ProbeLoop {
            runtimes: self.runtimes.clone(),
            probes: self.probes.clone(),
            last_adopt: Mutex::new(None),
        }
    }

    async fn execute_all(&self, cmds: Vec<Command>) {
        let results = self.run_all(cmds).await;
        self.report(results).await;
    }

    /// Run each command and return the results in order. A remembered
    /// command whose id was completed before is not run: its result is the
    /// one that was kept.
    async fn run_all(&self, cmds: Vec<Command>) -> Vec<CommandResult> {
        let mut results = Vec::with_capacity(cmds.len());
        for c in cmds {
            let id = c.id;
            let kind = c.kind;
            let remember = c.remember;
            if let Some(kept) = remember.then(|| self.done.get(id)).flatten() {
                tracing::info!(%id, %kind, "anvil: command was already done; answering with its result");
                results.push(kept);
                continue;
            }
            let r = match self.execute(c).await {
                Ok(payload) => CommandResult::ok(id).with_payload(payload),
                Err(e) => {
                    tracing::warn!(%id, %kind, error = %e, "anvil: command failed");
                    CommandResult::from_api_error(id, &e)
                }
            };
            if remember {
                self.done.record(&r);
            }
            results.push(r);
        }
        results
    }

    async fn report(&self, results: Vec<CommandResult>) {
        match self.client.report(&results).await {
            Ok(ack) if !ack.unknown.is_empty() => {
                tracing::warn!(unknown = ?ack.unknown, "anvil: server no longer knew some commands")
            }
            Ok(_) => {}
            Err(e) => {
                // A remembered result is sent again on the next turn of the
                // loop. Any other is dropped: the server times the command
                // out and whoever asked retries.
                tracing::warn!(error = %e, "anvil: reporting results failed");
                let mut unsent = self.unsent.lock().unwrap_or_else(|e| e.into_inner());
                unsent.extend(
                    results
                        .into_iter()
                        .filter(|r| self.done.get(r.id).is_some()),
                );
                let extra = unsent.len().saturating_sub(crate::done::KEEP_MAX);
                unsent.drain(..extra);
            }
        }
    }

    /// Send again the results no server has taken. A server that does not
    /// know an id has not been asked for that command yet; if it is, the
    /// command arrives again and is answered from the record.
    async fn resend(&self) {
        let waiting = std::mem::take(&mut *self.unsent.lock().unwrap_or_else(|e| e.into_inner()));
        if !waiting.is_empty() {
            self.report(waiting).await;
        }
    }

    /// Point `req.image` at a file on this host. The server sends the path
    /// the image has there; when that file is not here and the request says
    /// which catalog image it is, download it. A file that is here but not
    /// named for its format (the server's blob, on a host that runs both)
    /// is linked into the cache under such a name. A runtime that boots
    /// nothing from the file (mock, metal) is left alone.
    async fn localize_image(&self, req: &mut MachineRequest) -> ApiResult<()> {
        let Some(source) = &req.image_source else {
            return Ok(());
        };
        let reads_file = self
            .runtimes
            .get(req.kind)
            .is_some_and(|rt| !matches!(rt.name(), "mock" | "metal"));
        if !reads_file {
            return Ok(());
        }
        let local = std::path::Path::new(&req.image);
        let path = if local.is_file() {
            let named = local
                .extension()
                .is_some_and(|ext| ext == source.format.as_str());
            if named {
                return Ok(());
            }
            self.images.adopt(local, source).await?
        } else {
            self.images.ensure(&self.client, source).await?
        };
        req.image = path.to_string_lossy().into_owned();
        Ok(())
    }

    async fn execute(&self, c: Command) -> Result<serde_json::Value, ApiError> {
        match c.kind {
            CommandKind::StartMachine => {
                let mut req: MachineRequest = payload(c.payload)?;
                tracing::info!(machine = %req.id, name = %req.name, kind = ?req.kind, "anvil: start");
                if req.nics.iter().any(on_ovn) {
                    self.ovn.require_ready().await?;
                    // So the guest finds metadata on its first boot. A
                    // failure here does not stop the start.
                    self.meta.refresh(&self.client).await;
                }
                self.localize_image(&mut req).await?;
                let rt = self.runtimes.start(&req).await?;
                self.probes.install(
                    &req.id,
                    probe::probes_in_user_data(req.user_data.as_deref()),
                    Utc::now(),
                );
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
                self.probes.forget(&r.id);
                Ok(serde_json::Value::Null)
            }
            CommandKind::AttachVolume => {
                let req: VolumeAttach = payload(c.payload)?;
                tracing::info!(machine = %req.machine, volume = %req.volume, "anvil: attach volume");
                let attached = confirm_volume_attach(&self.runtimes, &req).await?;
                serde_json::to_value(attached).map_err(|e| ApiError::internal(e.to_string()))
            }
            CommandKind::DetachVolume => {
                let req: VolumeDetach = payload(c.payload)?;
                tracing::info!(machine = %req.machine, volume = %req.volume, "anvil: detach volume");
                // Unmount first. A busy mount must not be followed by an unmap.
                crate::runtime::volfs::release_volume(&req.volume).await?;
                self.runtimes.detach_volume(&req).await?;
                Ok(serde_json::Value::Null)
            }
            CommandKind::PlugPort => {
                let req: PortPlug = payload(c.payload)?;
                tracing::info!(machine = %req.machine, "anvil: plug port");
                if on_ovn(&req.nic) {
                    self.ovn.require_ready().await?;
                    self.meta.refresh(&self.client).await;
                }
                attach_port(&self.runtimes, &req).await?;
                Ok(serde_json::Value::Null)
            }
            CommandKind::UnplugPort => {
                let req: PortPlug = payload(c.payload)?;
                tracing::info!(machine = %req.machine, "anvil: unplug port");
                detach_port(&self.runtimes, &req).await?;
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
            CommandKind::ReadLogs => {
                let req: MachineRef = payload(c.payload)?;
                tracing::info!(machine = %req.id, "anvil: read logs");
                let text = read_machine_logs(&req.id).await?;
                serde_json::to_value(LogText { text })
                    .map_err(|e| ApiError::internal(e.to_string()))
            }
            CommandKind::Exec => {
                let req: ExecRequest = payload(c.payload)?;
                tracing::info!(machine = %req.id, container = %req.container, "anvil: exec");
                let timeout = pistack_rpc::proto::exec_timeout(req.timeout_secs)?;
                let env: Vec<(String, String)> =
                    req.env.into_iter().map(|e| (e.name, e.value)).collect();
                let out = crate::runtime::YoukiRuntime
                    .exec(
                        &req.id,
                        &req.container,
                        &req.command,
                        &env,
                        req.cwd.as_deref(),
                        timeout,
                        crate::runtime::youki::EXEC_OUTPUT_CAP,
                    )
                    .await?;
                serde_json::to_value(out).map_err(|e| ApiError::internal(e.to_string()))
            }
            CommandKind::Publish => Err(ApiError::not_implemented(
                "publish is handled by the edge host, not this agent",
            )),
        }
    }
}

/// Attach, then confirm only when the host path is real.
///
/// A pod is the attach youki recorded in `disks.json`. The agent mounts a
/// filesystem on that volume and confirms the mount directory. A VM local
/// file or LVM device is checked as the host path. An RBD image on a VM has
/// no host map and confirms with no path once the runtime attach succeeds.
/// A failed check undoes the attach.
async fn confirm_volume_attach(
    runtimes: &Runtimes,
    req: &VolumeAttach,
) -> ApiResult<VolumeAttached> {
    if !matches!(req.target, Some(DiskTarget::Rbd { .. })) {
        host_path_ready(&req.path)?;
    }
    let device = runtimes.attach_volume(req).await?;
    let recorded = recorded_disk_path(
        &crate::runtime::youki::state_dir(),
        &req.machine,
        &req.volume,
    );
    if let Some(path) = recorded.filter(|path| !path.is_empty()) {
        return confirm_pod_mount(runtimes, req, device, &path).await;
    }
    match confirmed_host_path(req).await {
        Ok(host_path) => Ok(VolumeAttached { device, host_path }),
        Err(err) => {
            let undo = VolumeDetach {
                machine: req.machine.clone(),
                volume: req.volume.clone(),
                path: req.path.clone(),
                device: Some(device),
                target: req.target.clone(),
            };
            if let Err(undo_err) = runtimes.detach_volume(&undo).await {
                tracing::warn!(
                    machine = %req.machine,
                    volume = %req.volume,
                    error = %undo_err,
                    "anvil: undoing attach after the host path check failed"
                );
            }
            Err(err)
        }
    }
}

/// Mount the pod volume and record the directory. A mount or format error
/// unmounts, detaches the loop device, and asks the runtime to unmap.
async fn confirm_pod_mount(
    runtimes: &Runtimes,
    req: &VolumeAttach,
    device: String,
    recorded: &str,
) -> ApiResult<VolumeAttached> {
    match crate::runtime::volfs::mount_for_pod(req, recorded).await {
        Ok(host_path) => Ok(VolumeAttached {
            device,
            host_path: Some(host_path),
        }),
        Err(err) => {
            if let Err(undo_err) = crate::runtime::volfs::release_volume(&req.volume).await {
                tracing::warn!(
                    machine = %req.machine,
                    volume = %req.volume,
                    error = %undo_err,
                    "anvil: unmounting a failed pod attach failed"
                );
            }
            let undo = VolumeDetach {
                machine: req.machine.clone(),
                volume: req.volume.clone(),
                path: req.path.clone(),
                device: Some(device),
                target: req.target.clone(),
            };
            if let Err(undo_err) = runtimes.detach_volume(&undo).await {
                tracing::warn!(
                    machine = %req.machine,
                    volume = %req.volume,
                    error = %undo_err,
                    "anvil: undoing a pod attach after the mount failed"
                );
            }
            Err(err)
        }
    }
}

async fn confirmed_host_path(req: &VolumeAttach) -> ApiResult<Option<String>> {
    let recorded = recorded_disk_path(
        &crate::runtime::youki::state_dir(),
        &req.machine,
        &req.volume,
    );
    let mapped = match &req.target {
        Some(DiskTarget::Rbd { pool, image, .. }) if recorded.is_none() => {
            match crate::runtime::rbdmap::mapped_device(pool, image).await {
                Ok(path) => path,
                Err(err) => {
                    tracing::warn!(
                        volume = %req.volume,
                        error = %err,
                        "anvil: rbd map list failed; confirming without a host path"
                    );
                    None
                }
            }
        }
        _ => None,
    };
    decide_host_path(req, recorded.as_deref(), mapped.as_deref())
}

/// `recorded` is the path youki stored for this volume. `mapped` is the
/// device `rbd` listed for the image. Either must exist before it is returned.
fn decide_host_path(
    req: &VolumeAttach,
    recorded: Option<&str>,
    mapped: Option<&str>,
) -> ApiResult<Option<String>> {
    if matches!(req.target, Some(DiskTarget::Rbd { .. })) {
        if let Some(path) = recorded {
            if path.is_empty() {
                return Err(ApiError::unprocessable("rbd mapped device path is missing"));
            }
            host_path_ready(path)?;
            return Ok(Some(path.to_owned()));
        }
        if let Some(path) = mapped {
            if path.is_empty() {
                return Err(ApiError::unprocessable("rbd mapped device path is missing"));
            }
            host_path_ready(path)?;
            return Ok(Some(path.to_owned()));
        }
        return Ok(None);
    }
    host_path_ready(&req.path)?;
    Ok(Some(req.path.clone()))
}

/// A local file or a device node. A directory is not a volume source.
fn host_path_ready(path: &str) -> ApiResult<()> {
    if path.is_empty() {
        return Err(ApiError::unprocessable("host path is missing"));
    }
    let p = std::path::Path::new(path);
    if path.contains('\0')
        || !p.is_absolute()
        || p.components()
            .any(|c| matches!(c, std::path::Component::ParentDir))
    {
        return Err(ApiError::bad_request(format!(
            "host path {path} must be an absolute path"
        )));
    }
    match std::fs::metadata(p) {
        Ok(meta) if meta.is_dir() => Err(ApiError::unprocessable(format!(
            "host path {path} is a directory"
        ))),
        Ok(_) => Ok(()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Err(ApiError::unprocessable(
            format!("host path {path} does not exist"),
        )),
        Err(err) => Err(ApiError::internal(format!("host path {path}: {err}"))),
    }
}

/// Path youki recorded for this volume in `disks.json`, if the bundle has one.
fn recorded_disk_path(state: &std::path::Path, machine: &Id, volume: &Id) -> Option<String> {
    let name = machine.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return None;
    }
    let bytes = std::fs::read(state.join(name).join("disks.json")).ok()?;
    let disks: Vec<serde_json::Value> = serde_json::from_slice(&bytes).ok()?;
    let vol = volume.to_string();
    disks.into_iter().find_map(|disk| {
        if disk.get("volume").and_then(|v| v.as_str()) != Some(vol.as_str()) {
            return None;
        }
        disk.get("path").and_then(|p| p.as_str()).map(str::to_owned)
    })
}

/// Cloud Hypervisor is the VM runtime only when this host selected it.
/// libvirt keeps port plug on every other host, including the mock.
fn cloud_vm(runtimes: &Runtimes) -> bool {
    runtimes
        .get(MachineKind::Vm)
        .is_some_and(|rt| rt.name() == "cloud-hypervisor")
}

/// True when fabric bound this NIC to the OVN integration bridge.
fn on_ovn(nic: &pistack_rpc::proto::NicAttachment) -> bool {
    nic.binding.as_ref().is_some_and(|b| b.vif_type == "ovs")
}

async fn attach_port(runtimes: &Runtimes, req: &PortPlug) -> Result<(), ApiError> {
    if cloud_vm(runtimes) {
        crate::runtime::cloud::plug_port(req).await
    } else {
        crate::runtime::libvirt::plug_port(req).await
    }
}

async fn detach_port(runtimes: &Runtimes, req: &PortPlug) -> Result<(), ApiError> {
    if cloud_vm(runtimes) {
        crate::runtime::cloud::unplug_port(req).await
    } else {
        crate::runtime::libvirt::unplug_port(req).await
    }
}

const LOG_CAP: usize = 256 * 1024;

fn youki_state_dir() -> std::path::PathBuf {
    crate::runtime::youki::state_dir()
}

fn bundle_name(id: &pistack_types::Id) -> Result<String, ApiError> {
    let name = id.to_string();
    if name.is_empty() || !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        return Err(ApiError::bad_request(format!(
            "machine id {name} cannot name a log"
        )));
    }
    Ok(name)
}

/// Youki bundle log (`log`, then `console.log`) when one exists, otherwise
/// the libvirt console log for the same machine. Lines that carry user data,
/// ssh keys, or passwords are removed.
async fn read_machine_logs(id: &pistack_types::Id) -> Result<String, ApiError> {
    read_machine_logs_in(
        id,
        &youki_state_dir(),
        &crate::runtime::libvirt::images_dir(),
    )
    .await
}

async fn read_machine_logs_in(
    id: &pistack_types::Id,
    youki_state: &std::path::Path,
    libvirt_root: &std::path::Path,
) -> Result<String, ApiError> {
    let name = bundle_name(id)?;
    let bundle = youki_state.join(&name);
    let vm = libvirt_root
        .join("machines")
        .join(format!("pistack-{name}"));
    let mut empty = None;
    for (path, jail) in [
        (bundle.join("log"), bundle.clone()),
        (bundle.join("console.log"), bundle.clone()),
    ] {
        if let Some(text) = consider_log(&path, &jail, &mut empty).await? {
            return Ok(text);
        }
    }
    // Pod containers keep their own `log` under the sandbox bundle. The
    // sandbox pause process does not write one.
    if let Some(text) = read_container_logs(&bundle).await? {
        if !text.is_empty() {
            return Ok(scrub_log(&text));
        }
        if empty.is_none() {
            empty = Some(String::new());
        }
    }
    if let Some(text) = consider_log(&vm.join("console.log"), &vm, &mut empty).await? {
        return Ok(text);
    }
    if let Some(text) = empty {
        return Ok(text);
    }
    Err(ApiError::not_found(format!("no log for machine {id}")))
}

async fn consider_log(
    path: &std::path::Path,
    jail: &std::path::Path,
    empty: &mut Option<String>,
) -> Result<Option<String>, ApiError> {
    if let Some(text) = read_jailed_log(path, jail).await? {
        if !text.is_empty() {
            return Ok(Some(scrub_log(&text)));
        }
        if empty.is_none() {
            *empty = Some(scrub_log(&text));
        }
    }
    Ok(None)
}

async fn read_container_logs(bundle: &std::path::Path) -> Result<Option<String>, ApiError> {
    let root = bundle.join("containers");
    let mut rd = match tokio::fs::read_dir(&root).await {
        Ok(rd) => rd,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ApiError::unavailable(format!("reading log: {e}"))),
    };
    let mut dirs = Vec::new();
    while let Some(ent) = rd
        .next_entry()
        .await
        .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?
    {
        let ft = ent
            .file_type()
            .await
            .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?;
        if !ft.is_dir() {
            continue;
        }
        let name = ent.file_name();
        let name = name.to_string_lossy();
        if name.is_empty() || !name.chars().all(|c| c.is_ascii_digit()) {
            continue;
        }
        dirs.push(ent.path());
    }
    dirs.sort_by_key(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<u64>().ok())
            .unwrap_or(u64::MAX)
    });
    let mut combined = String::new();
    let mut saw = false;
    for dir in dirs {
        let Some(text) = read_jailed_log(&dir.join("log"), &dir).await? else {
            continue;
        };
        saw = true;
        if text.is_empty() {
            continue;
        }
        if !combined.is_empty() && !combined.ends_with('\n') {
            combined.push('\n');
        }
        combined.push_str(&text);
    }
    if saw {
        Ok(Some(combined))
    } else {
        Ok(None)
    }
}

async fn read_jailed_log(
    path: &std::path::Path,
    jail: &std::path::Path,
) -> Result<Option<String>, ApiError> {
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_file() => {}
        Ok(_) => return Ok(None),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ApiError::unavailable(format!("reading log: {e}"))),
    }
    let canon = tokio::fs::canonicalize(path)
        .await
        .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?;
    let jail_canon = match tokio::fs::canonicalize(jail).await {
        Ok(p) => p,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(ApiError::unavailable(format!("reading log: {e}"))),
    };
    if !canon.starts_with(&jail_canon) {
        return Ok(None);
    }
    let mut file = tokio::fs::File::open(&canon)
        .await
        .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?;
    let len = file
        .metadata()
        .await
        .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?
        .len();
    if len > LOG_CAP as u64 {
        use tokio::io::AsyncSeekExt;
        file.seek(std::io::SeekFrom::Start(len - LOG_CAP as u64))
            .await
            .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?;
    }
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    file.read_to_end(&mut buf)
        .await
        .map_err(|e| ApiError::unavailable(format!("reading log: {e}")))?;
    Ok(Some(tail_log_bytes(&buf, LOG_CAP)))
}

fn tail_log_bytes(bytes: &[u8], cap: usize) -> String {
    if cap == 0 || bytes.is_empty() {
        return String::new();
    }
    let slice = if bytes.len() > cap {
        let mut start = bytes.len() - cap;
        while start < bytes.len() && (bytes[start] & 0b1100_0000) == 0b1000_0000 {
            start += 1;
        }
        &bytes[start..]
    } else {
        bytes
    };
    String::from_utf8_lossy(slice).into_owned()
}

fn secret_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "ssh-ed25519",
        "ssh-rsa",
        "ssh-dss",
        "ecdsa-sha2-",
        "sk-ssh-",
        "sk-ecdsa-",
        "begin openssh private key",
        "begin rsa private key",
        "begin dsa private key",
        "begin ec private key",
        "begin private key",
        "begin openssh",
        "user_data",
        "user-data",
        "ssh_authorized_keys",
        "authorized_keys",
    ];
    MARKERS.iter().any(|m| lower.contains(m))
}

fn secret_key_at(lower: &str, i: usize) -> Option<usize> {
    let rest = &lower[i..];
    let key = if rest.starts_with("password") {
        "password"
    } else if rest.starts_with("passwd") {
        "passwd"
    } else {
        return None;
    };
    if i > 0 && lower.as_bytes()[i - 1].is_ascii_alphanumeric() {
        return None;
    }
    Some(key.len())
}

fn redact_secrets(line: &str) -> String {
    let lower = line.to_ascii_lowercase();
    let bytes = line.as_bytes();
    let mut out = String::with_capacity(line.len());
    let mut i = 0;
    while i < bytes.len() {
        if let Some(key_len) = secret_key_at(&lower, i) {
            let after = i + key_len;
            let rest = &lower[after..];
            let spaces = rest.len() - rest.trim_start_matches(' ').len();
            let trimmed = &rest[spaces..];
            if trimmed.starts_with(':') || trimmed.starts_with('=') {
                out.push_str(&line[i..after]);
                out.push_str(&line[after..after + spaces]);
                let sep_at = after + spaces;
                out.push(bytes[sep_at] as char);
                let mut j = sep_at + 1;
                while j < bytes.len() && bytes[j] == b' ' {
                    out.push(' ');
                    j += 1;
                }
                out.push_str("***");
                while j < bytes.len() && !bytes[j].is_ascii_whitespace() {
                    j += 1;
                }
                i = j;
                continue;
            }
        }
        let ch = line[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

fn scrub_log(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for line in text.split_inclusive('\n') {
        let (body, nl) = match line.strip_suffix('\n') {
            Some(body) => (body, "\n"),
            None => (line, ""),
        };
        if secret_line(body) {
            continue;
        }
        out.push_str(&redact_secrets(body));
        out.push_str(nl);
    }
    out
}

fn payload<T: serde::de::DeserializeOwned>(v: serde_json::Value) -> Result<T, ApiError> {
    serde_json::from_value(v)
        .map_err(|e| ApiError::bad_request(format!("bad command payload: {e}")))
}

/// Longest a heartbeat waits for the runtimes to say what machines use.
const USAGE_TIMEOUT: Duration = Duration::from_secs(5);

struct HeartbeatTask {
    client: AgentClient,
    runtimes: Runtimes,
    resources: Resources,
    /// Whose filesystem the host usage reports on.
    data_dir: std::path::PathBuf,
    probes: Arc<ProbeBook>,
    ovn: Arc<OvnHost>,
    ovn_watch: Arc<AtomicBool>,
    meta: Arc<MetaNets>,
}

/// Runs due probes about once a second. An exec probe asks the runtime that
/// holds the pod to run the argv. youki uses its exec helper. Any other
/// runtime reports the probe unsupported. Adoption from the runtime (so a
/// restarted agent keeps checking) is slower, and only while nothing is
/// tracked yet, so a host with no probes does not list machines every tick.
struct ProbeLoop {
    runtimes: Runtimes,
    probes: Arc<ProbeBook>,
    last_adopt: Mutex<Option<Instant>>,
}

const ADOPT_EVERY: Duration = Duration::from_secs(15);

impl ProbeLoop {
    async fn run(self, stop: CancellationToken) {
        let mut tick = tokio::time::interval(Duration::from_secs(1));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tick.tick() => {}
            }
            self.pass().await;
        }
    }

    fn adopt_due(&self) -> bool {
        let guard = self.last_adopt.lock().unwrap_or_else(|e| e.into_inner());
        match *guard {
            None => true,
            Some(at) => at.elapsed() >= ADOPT_EVERY,
        }
    }

    fn mark_adopted(&self) {
        *self.last_adopt.lock().unwrap_or_else(|e| e.into_inner()) = Some(Instant::now());
    }

    async fn pass(&self) {
        let adopt = !self.probes.has_tracks() && self.adopt_due();
        if !self.probes.has_tracks() && !adopt {
            return;
        }
        let machines = self.runtimes.list().await;
        let now = Utc::now();
        if adopt {
            for machine in &machines {
                if machine.power != pistack_rpc::proto::PowerState::On
                    || self.probes.contains(&machine.id)
                {
                    continue;
                }
                let specs = self.runtimes.container_probes(&machine.id).await;
                if !specs.is_empty() {
                    self.probes.install(&machine.id, specs, now);
                }
            }
            self.mark_adopted();
        }
        if machines.is_empty() || !self.probes.has_tracks() {
            return;
        }
        let live: BTreeSet<Id> = machines.iter().map(|m| m.id.clone()).collect();
        self.probes.retain(&live);
        let targets: Vec<ProbeTarget> = machines
            .iter()
            .map(|m| ProbeTarget {
                id: m.id.clone(),
                power_on: m.power == pistack_rpc::proto::PowerState::On,
                address: m.addresses.iter().find(|a| !a.is_empty()).cloned(),
            })
            .collect();
        let due = self.probes.due(&targets, now);
        for check in due {
            let outcome = probe::run(&check, |argv, timeout| {
                let runtimes = self.runtimes.clone();
                let machine = check.machine.clone();
                let container = check.container.clone();
                let argv = argv.to_vec();
                async move {
                    probe::reply_from_exec(
                        runtimes
                            .command_probe(&machine, &container, &argv, timeout)
                            .await,
                    )
                }
            })
            .await;
            if !outcome.success && !outcome.unsupported {
                tracing::debug!(
                    machine = %check.machine,
                    container = %check.container,
                    kind = %check.kind,
                    message = %outcome.message,
                    "anvil: probe failed"
                );
            }
            self.probes.record(&check, &outcome);
        }
    }
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
        let usage_every = Duration::from_secs(pistack_rpc::proto::USAGE_INTERVAL_SECS);
        let mut usage_sent: Option<std::time::Instant> = None;
        // The first tick fires immediately; a heartbeat right after hello
        // delivers the machine list without waiting a full interval.
        loop {
            tokio::select! {
                _ = stop.cancelled() => return,
                _ = tick.tick() => {}
            }
            let machines = self.runtimes.list().await;
            let mut rows = Vec::with_capacity(machines.len());
            for machine in machines {
                let id = machine.id.clone();
                let mut row = report(machine);
                row.probes = self.probes.reports(&id);
                rows.push(row);
            }
            // Usage goes with a heartbeat, but not more often than it is
            // worth measuring when the interval is short.
            let usage = if usage_sent.is_none_or(|at| at.elapsed() >= usage_every) {
                usage_sent = Some(std::time::Instant::now());
                // A runtime that does not answer must not hold the
                // heartbeat back; the host figures go without it.
                let machines = tokio::time::timeout(USAGE_TIMEOUT, self.runtimes.usage())
                    .await
                    .unwrap_or_else(|_| {
                        tracing::warn!("anvil: runtimes did not report usage in time");
                        Vec::new()
                    });
                Some(pistack_rpc::proto::UsageReport {
                    host: crate::usage::host_usage(&self.data_dir),
                    machines,
                })
            } else {
                None
            };
            let hb = Heartbeat {
                node_name: self.client.node_name().to_owned(),
                resources_free: self.resources,
                machines: rows,
                at: Utc::now(),
                ovn: if self.ovn_watch.load(Ordering::Relaxed) {
                    Some(self.ovn.check().await)
                } else {
                    None
                },
                usage,
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
            // Follow the guests: a network that gained its first guest
            // here gets a metadata namespace, one that lost its last
            // guest loses it.
            if self.ovn_watch.load(Ordering::Relaxed) {
                self.meta.refresh(&self.client).await;
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
        message: m.message,
        resources: None,
        addresses: m.addresses,
        power: Some(m.power),
        probes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MachineRuntime;
    use async_trait::async_trait;

    #[test]
    fn plug_uses_cloud_only_when_that_runtime_owns_vms() {
        let cloud: crate::runtime::SharedRuntime = Arc::new(crate::runtime::CloudRuntime);
        let mut rts = Runtimes::new();
        rts.insert(MachineKind::Vm, cloud);
        assert!(cloud_vm(&rts));

        let lib: crate::runtime::SharedRuntime = Arc::new(crate::runtime::LibvirtRuntime);
        let mut rts = Runtimes::new();
        rts.insert(MachineKind::Vm, lib);
        assert!(!cloud_vm(&rts));

        let mock: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::default());
        assert!(!cloud_vm(&Runtimes::all(mock)));
    }

    fn issued_pki(store: &pistack_store::Store, cn: &str) -> Pki {
        keep::Client::open(store)
            .unwrap()
            .issue_client_cert(cn)
            .unwrap()
    }

    #[test]
    fn cert_node_name_reads_the_enrolled_common_name() {
        let store = pistack_store::Store::in_memory().unwrap();
        let pki = issued_pki(&store, "rack1-n3");
        assert_eq!(cert_node_name(&pki.cert_pem).as_deref(), Some("rack1-n3"));
        assert_eq!(cert_node_name("not a pem"), None);
    }

    #[test]
    fn persisted_cert_is_kept_only_for_its_own_node() {
        assert_eq!(persisted_cert_use(Some("n1"), "n1", false), CertUse::Keep);
        assert_eq!(persisted_cert_use(Some("n1"), "n1", true), CertUse::Keep);
        assert_eq!(
            persisted_cert_use(Some("n1"), "n2", true),
            CertUse::Reenroll(Some("n1".into()))
        );
        assert_eq!(
            persisted_cert_use(Some("n1"), "n2", false),
            CertUse::Refuse(Some("n1".into()))
        );
        assert_eq!(persisted_cert_use(None, "n2", false), CertUse::Refuse(None));
    }

    #[test]
    fn identity_survives_restart_and_adopts_an_existing_cert() {
        let store = pistack_store::Store::in_memory().unwrap();
        // An agent enrolled before node.name existed: the certificate's
        // name wins over whatever the host is called today.
        let dir = tempfile::tempdir().unwrap();
        save_agent_pki(
            &dir.path().join("agent"),
            &issued_pki(&store, "enrolled-n1"),
        )
        .unwrap();
        let id = agent_identity(dir.path(), None).unwrap();
        assert_eq!(id.name, "enrolled-n1");
        assert_eq!(id.source, node::NodeNameSource::Adopted);
        // Restart: same name, now from the file.
        let id = agent_identity(dir.path(), None).unwrap();
        assert_eq!(id.name, "enrolled-n1");
        assert_eq!(id.source, node::NodeNameSource::File);
        // An explicit rename wins and sticks.
        let id = agent_identity(dir.path(), Some("renamed")).unwrap();
        assert_eq!(id.name, "renamed");
        assert_eq!(id.replaced.as_deref(), Some("enrolled-n1"));
        assert_eq!(agent_identity(dir.path(), None).unwrap().name, "renamed");
    }

    #[tokio::test]
    async fn rejoin_refreshes_the_cert_but_keeps_the_name() {
        let store = pistack_store::Store::in_memory().unwrap();
        keep::Client::open(&store).unwrap();
        let (_, first) = keep::create_join_token(&store, "a", "admin", false, None).unwrap();
        let (_, second) = keep::create_join_token(&store, "b", "admin", false, None).unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = axum::Router::new().nest(
            RPC_PREFIX,
            pistack_rpc::router_with_auth(
                pistack_rpc::CommandQueue::default(),
                store.clone(),
                pistack_rpc::RpcAuth::open(),
            ),
        );
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });

        let dir = tempfile::tempdir().unwrap();
        let opts = |token: Option<&str>| AgentOpts {
            join: format!("http://{addr}"),
            token: token.map(str::to_owned),
            data_dir: dir.path().to_owned(),
        };
        let cert = || std::fs::read_to_string(dir.path().join("agent/cert.pem")).unwrap();

        // First start: resolve, enroll, persist.
        let node = agent_identity(dir.path(), None).unwrap().name;
        let client = connect_and_enroll(&opts(Some(&first)), &node)
            .await
            .unwrap();
        assert_eq!(client.node_name(), node);
        let enrolled = cert();
        assert_eq!(cert_node_name(&enrolled).as_deref(), Some(node.as_str()));

        // Restart without a token: same name, same certificate.
        assert_eq!(agent_identity(dir.path(), None).unwrap().name, node);
        let client = connect_and_enroll(&opts(None), &node).await.unwrap();
        assert_eq!(client.node_name(), node);
        assert_eq!(cert(), enrolled);

        // Rejoin with a fresh token after the certificate is lost: a new
        // certificate, the same name.
        std::fs::remove_file(dir.path().join("agent/cert.pem")).unwrap();
        assert_eq!(agent_identity(dir.path(), None).unwrap().name, node);
        let client = connect_and_enroll(&opts(Some(&second)), &node)
            .await
            .unwrap();
        assert_eq!(client.node_name(), node);
        let refreshed = cert();
        assert_ne!(refreshed, enrolled);
        assert_eq!(cert_node_name(&refreshed).as_deref(), Some(node.as_str()));

        // A certificate for another node is refused without a token.
        let err = connect_and_enroll(&opts(None), "someone-else")
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("someone-else") && err.contains(&node), "{err}");
        server.abort();
    }

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
                ovn: None,
                usage: None,
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

    #[test]
    fn scrub_drops_keys_and_user_data_and_redacts_passwords() {
        let raw = "\
hello
password: hunter2
passwd=secret
Failed password for invalid user
ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAISecret demo
user_data: cloud-config
authorized_keys: present
still here
";
        let text = scrub_log(raw);
        assert!(text.contains("hello"));
        assert!(text.contains("still here"));
        assert!(text.contains("password: ***"));
        assert!(text.contains("passwd=***"));
        assert!(text.contains("Failed password for invalid user"));
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("secret"));
        assert!(!text.contains("ssh-ed25519"));
        assert!(!text.contains("AAAAC3"));
        assert!(!text.contains("user_data"));
        assert!(!text.contains("cloud-config"));
        assert!(!text.contains("authorized_keys"));
        let tail = tail_log_bytes("abcdefghijklmnop".as_bytes(), 4);
        assert_eq!(tail, "mnop");
        let split = tail_log_bytes(&[b'a', 0xc3, 0xa9, b'b'], 2);
        assert!(
            !split.contains('\u{fffd}') || split.ends_with('b') || split == "éb" || split == "b"
        );
        assert!(split.ends_with('b'));
    }

    #[tokio::test]
    async fn read_logs_prefers_the_youki_bundle_and_skips_symlinks() {
        let root = std::env::temp_dir().join(format!(
            "pistack-logs-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let id = pistack_types::Id::parse(
            pistack_types::kind::MACHINE,
            "mach-018f2c3a-7b6e-7a11-8c22-123456789abc",
        )
        .unwrap();
        let name = id.to_string();
        let youki = root.join("youki");
        let images = root.join("images");
        let bundle = youki.join(&name);
        let vm = images.join("machines").join(format!("pistack-{name}"));
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::create_dir_all(&vm).unwrap();
        std::fs::write(bundle.join("log"), "from-youki\npassword: hunter2\n").unwrap();
        std::fs::write(vm.join("console.log"), "from-kvm\n").unwrap();
        let text = read_machine_logs_in(&id, &youki, &images).await.unwrap();
        assert!(text.contains("from-youki"));
        assert!(text.contains("password: ***"));
        assert!(!text.contains("hunter2"));
        assert!(!text.contains("from-kvm"));

        std::fs::remove_file(bundle.join("log")).unwrap();
        let text = read_machine_logs_in(&id, &youki, &images).await.unwrap();
        assert_eq!(text, "from-kvm\n");

        let outside = root.join("secret.txt");
        std::fs::write(&outside, "ssh-ed25519 AAAALEAK demo\n").unwrap();
        std::os::unix::fs::symlink(&outside, bundle.join("log")).unwrap();
        std::fs::remove_file(vm.join("console.log")).unwrap();
        let err = read_machine_logs_in(&id, &youki, &images)
            .await
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::NotFound);
        assert!(!err.message.contains("AAAALEAK"));
        assert!(!err.message.contains("ssh-ed25519"));

        std::fs::write(vm.join("console.log"), "").unwrap();
        let text = read_machine_logs_in(&id, &youki, &images).await.unwrap();
        assert_eq!(text, "");

        std::fs::remove_file(vm.join("console.log")).unwrap();
        let c0 = bundle.join("containers").join("0");
        let c1 = bundle.join("containers").join("1");
        std::fs::create_dir_all(&c0).unwrap();
        std::fs::create_dir_all(&c1).unwrap();
        std::fs::write(c0.join("log"), "from-c0\n").unwrap();
        std::fs::write(c1.join("log"), "password: hunter2\nfrom-c1\n").unwrap();
        let text = read_machine_logs_in(&id, &youki, &images).await.unwrap();
        assert!(text.find("from-c0").unwrap() < text.find("from-c1").unwrap());
        assert!(text.contains("password: ***"));
        assert!(!text.contains("hunter2"));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[tokio::test]
    async fn read_logs_command_rejects_a_bad_payload() {
        let client = AgentClient::new("http://127.0.0.1:1", "n1").unwrap();
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::new());
        let agent = Agent {
            client,
            runtimes: Runtimes::all(rt),
            facts: facts::gather(),
            probes: Arc::new(ProbeBook::new()),
            images: ImageCache::new(std::path::Path::new("/nonexistent")),
            ovn: Arc::new(OvnHost::system()),
            ovn_watch: Arc::new(AtomicBool::new(false)),
            meta: Arc::new(MetaNets::system()),
            underlay: None,
            done: Arc::new(Completed::in_memory()),
            unsent: Mutex::new(Vec::new()),
        };
        let bad = agent
            .execute(Command {
                id: uuid::Uuid::now_v7(),
                kind: CommandKind::ReadLogs,
                payload: serde_json::json!({}),
                issued_at: Utc::now(),
                remember: false,
            })
            .await
            .unwrap_err();
        assert_eq!(bad.code, pistack_types::error::ErrorCode::BadRequest);
        let bad = agent
            .execute(Command {
                id: uuid::Uuid::now_v7(),
                kind: CommandKind::PlugPort,
                payload: serde_json::json!({"machine": "nope"}),
                issued_at: Utc::now(),
                remember: false,
            })
            .await
            .unwrap_err();
        assert_eq!(bad.code, pistack_types::error::ErrorCode::BadRequest);
        let bad = agent
            .execute(Command {
                id: uuid::Uuid::now_v7(),
                kind: CommandKind::UnplugPort,
                payload: serde_json::json!(null),
                issued_at: Utc::now(),
                remember: false,
            })
            .await
            .unwrap_err();
        assert_eq!(bad.code, pistack_types::error::ErrorCode::BadRequest);

        let id = pistack_types::Id::new(pistack_types::kind::MACHINE);
        match agent
            .execute(Command {
                id: uuid::Uuid::now_v7(),
                kind: CommandKind::ReadLogs,
                payload: serde_json::json!({"id": id.to_string()}),
                issued_at: Utc::now(),
                remember: false,
            })
            .await
        {
            Err(e) => {
                assert_eq!(e.code, pistack_types::error::ErrorCode::NotFound);
                assert!(!e.message.contains("ssh-"));
            }
            Ok(v) => {
                let n = v
                    .get("text")
                    .and_then(|t| t.as_str())
                    .map(|s| s.len())
                    .unwrap_or(0);
                panic!("expected no log for a fresh id, got {n} bytes");
            }
        }
    }

    #[tokio::test]
    async fn exec_command_round_trips_through_the_queue() {
        let _lock = crate::runtime::youki::youki_env_lock().lock().await;
        let tmp = std::env::temp_dir().join(format!(
            "pistack-agent-exec-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).unwrap();
        let log = tmp.join("youki.log");
        let bin = tmp.join("youki");
        let quoted = format!("'{}'", log.display().to_string().replace('\'', "'\\''"));
        let script = format!(
            "#!/bin/sh\n\
             printf '%s\\n' \"$@\" >> {quoted}\n\
             printf '\\n' >> {quoted}\n\
             if [ \"$1\" = state ]; then\n\
               printf '%s\\n' '{{\"pid\":4242,\"status\":\"running\"}}'\n\
               exit 0\n\
             fi\n\
             if [ \"$1\" = exec ]; then\n\
               printf 'agent-out\\n'\n\
               printf 'agent-err\\n' >&2\n\
               exit 9\n\
             fi\n\
             exit 0\n"
        );
        std::fs::write(&bin, script).unwrap();
        let mut perms = std::fs::metadata(&bin).unwrap().permissions();
        std::os::unix::fs::PermissionsExt::set_mode(&mut perms, 0o755);
        std::fs::set_permissions(&bin, perms).unwrap();
        let image = tmp.join("image");
        std::fs::create_dir_all(&image).unwrap();
        std::fs::write(image.join("marker"), "keep").unwrap();
        let state = tmp.join("state");
        let prev_bin = std::env::var_os("PISTACK_YOUKI_BIN");
        let prev_state = std::env::var_os("PISTACK_YOUKI_STATE");
        std::env::set_var("PISTACK_YOUKI_BIN", &bin);
        std::env::set_var("PISTACK_YOUKI_STATE", &state);
        struct Restore(
            Option<std::ffi::OsString>,
            Option<std::ffi::OsString>,
            std::path::PathBuf,
        );
        impl Drop for Restore {
            fn drop(&mut self) {
                match self.0.take() {
                    Some(v) => std::env::set_var("PISTACK_YOUKI_BIN", v),
                    None => std::env::remove_var("PISTACK_YOUKI_BIN"),
                }
                match self.1.take() {
                    Some(v) => std::env::set_var("PISTACK_YOUKI_STATE", v),
                    None => std::env::remove_var("PISTACK_YOUKI_STATE"),
                }
                let _ = std::fs::remove_dir_all(&self.2);
            }
        }
        let _restore = Restore(prev_bin, prev_state, tmp.clone());

        let image_arg = image.to_str().unwrap();
        let request = crate::runtime::MachineRequest {
            id: pistack_types::Id::new(pistack_types::kind::MACHINE),
            name: "web".into(),
            kind: crate::runtime::MachineKind::Container,
            image: "not-a-path".into(),
            image_source: None,
            resources: Resources {
                vcpus: 1,
                memory_mib: 128,
                disk_gib: 1,
                gpus: 0,
            },
            networks: vec![],
            nics: vec![crate::runtime::NicAttachment {
                address: "10.9.0.2".into(),
                ..Default::default()
            }],
            volumes: vec![],
            user_data: Some(
                serde_json::json!({
                    "kind": "PodSandbox",
                    "containers": [{
                        "name": "web",
                        "image": image_arg,
                        "command": ["/usr/bin/web"],
                        "env": [{"name": "SECRET", "value_from": {"secret": "do-not-log-user-data"}}]
                    }]
                })
                .to_string(),
            ),
            ssh_keys: vec!["do-not-log-ssh-key".into()],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        };
        crate::runtime::YoukiRuntime.start(&request).await.unwrap();

        let client = AgentClient::new("http://127.0.0.1:1", "n1").unwrap();
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::new());
        let agent = Agent {
            client,
            runtimes: Runtimes::all(rt),
            facts: facts::gather(),
            probes: Arc::new(ProbeBook::new()),
            images: ImageCache::new(std::path::Path::new("/nonexistent")),
            ovn: Arc::new(OvnHost::system()),
            ovn_watch: Arc::new(AtomicBool::new(false)),
            meta: Arc::new(MetaNets::system()),
            underlay: None,
            done: Arc::new(Completed::in_memory()),
            unsent: Mutex::new(Vec::new()),
        };
        let queue = pistack_rpc::CommandQueue::new(pistack_rpc::QueueConfig {
            command_timeout: Duration::from_secs(5),
            capacity: 4,
            heartbeat_interval: Duration::from_secs(30),
            ovn: None,
        });

        let ran = queue_exec(
            &queue,
            &agent,
            serde_json::json!({
                "id": request.id.to_string(),
                "container": "web",
                "command": ["/bin/echo", "hi"],
                "timeout_secs": 5
            }),
        )
        .await
        .unwrap();
        let out: pistack_rpc::ExecOutput = serde_json::from_value(ran).unwrap();
        assert_eq!(out.exit_code, 9);
        assert_eq!(out.stdout, "agent-out\n");
        assert_eq!(out.stderr, "agent-err\n");
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(!logged.contains("do-not-log-user-data"), "{logged}");
        assert!(!logged.contains("do-not-log-ssh-key"), "{logged}");

        let bad = queue_exec(&queue, &agent, serde_json::json!({}))
            .await
            .unwrap_err();
        assert_eq!(bad.code, pistack_types::error::ErrorCode::BadRequest);

        let missing = queue_exec(
            &queue,
            &agent,
            serde_json::json!({
                "id": request.id.to_string(),
                "container": "missing",
                "command": ["/bin/true"],
                "timeout_secs": 5
            }),
        )
        .await
        .unwrap_err();
        assert_eq!(missing.code, pistack_types::error::ErrorCode::NotFound);
        assert!(missing.message.contains("missing"), "{missing}");
    }

    fn ovn_start(name: &str) -> serde_json::Value {
        serde_json::json!({
            "id": Id::new(pistack_types::kind::MACHINE).to_string(),
            "name": name,
            "image": "ubuntu-24.04",
            "nics": [{
                "network_id": "net-01a102de-3f33-73bf-86f7-57fe3b47f1ea",
                "mac": "52:54:00:aa:bb:cc",
                "address": "10.90.0.10",
                "prefix": 24,
                "gateway": "10.90.0.1",
                "routed": true,
                "binding": {
                    "vif_type": "ovs",
                    "vif_details": {"bridge": "br-int", "iface_id": "port-1"}
                }
            }]
        })
    }

    #[tokio::test]
    async fn the_hello_answer_sets_up_ovn_and_heartbeats_report_the_chassis() {
        use crate::ovnhost::tests::{cfg, tools, FakeSwitch};
        use pistack_rpc::proto::AgentEvent;

        let queue = pistack_rpc::CommandQueue::new(pistack_rpc::QueueConfig {
            command_timeout: Duration::from_secs(10),
            capacity: 8,
            heartbeat_interval: Duration::from_secs(1),
            ovn: Some(cfg()),
        });
        let mut events = queue.subscribe_agent_events();
        let app = axum::Router::new().nest("/rpc", pistack_rpc::router(queue.clone()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/rpc", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
                .unwrap();
        });

        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        let ovn = Arc::new(OvnHost::with(switch.clone(), tools(false)));
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::new());
        let facts = HostFacts {
            node_name: "node-a".into(),
            resources: Resources::default(),
            capabilities: vec![],
        };
        let agent = tokio::spawn(run_agent_on(
            AgentClient::new(base, "node-a").unwrap(),
            Runtimes::all(rt),
            facts,
            ovn,
            Arc::new(MetaNets::with(
                Arc::new(crate::ovnmeta::tests::FakeHost::default()),
                Arc::new(crate::ovnmeta::tests::FakeListen::default()),
                true,
            )),
            Some("10.66.66.5".into()),
            shutdown.clone(),
        ));

        let report = tokio::time::timeout(Duration::from_secs(10), async {
            loop {
                if let Ok(AgentEvent::Heartbeat(hb)) = events.recv().await {
                    if let Some(report) = hb.ovn {
                        return report;
                    }
                }
            }
        })
        .await
        .expect("a heartbeat with an ovn report");
        assert!(report.ready, "{}", report.message);
        assert_eq!(report.chassis, "node-a");
        assert_eq!(
            switch.get("ovn-remote").as_deref(),
            Some("tcp:10.66.66.1:6642")
        );
        assert_eq!(switch.get("ovn-encap-ip").as_deref(), Some("10.66.66.5"));
        assert!(switch.has_bridge("br-int"));

        let started = queue
            .enqueue("node-a", CommandKind::StartMachine, ovn_start("up"))
            .unwrap()
            .wait()
            .await
            .unwrap();
        assert!(started.ok, "{}", started.message);

        // ovn-controller dies: the next machine is refused, not plugged
        // into a bridge nothing programs.
        switch.controller(None);
        let refused = queue
            .enqueue("node-a", CommandKind::StartMachine, ovn_start("down"))
            .unwrap()
            .wait()
            .await
            .unwrap()
            .into_api_result()
            .unwrap_err();
        assert_eq!(refused.code, pistack_types::error::ErrorCode::Unavailable);
        assert!(refused.message.contains("not ready for OVN"), "{refused}");
        assert!(refused.message.contains("ovn-controller"), "{refused}");

        shutdown.cancel();
        agent.await.unwrap().unwrap();
    }

    #[tokio::test]
    async fn an_ovn_host_serves_metadata_for_the_networks_the_server_lists() {
        use crate::ovnhost::tests::{tools, FakeSwitch};
        use crate::ovnmeta::tests::{get, net, FakeHost, FakeListen, NET_A, PORT_A};
        use pistack_rpc::proto::{MetadataAnswer, MetadataNetwork, MetadataQuery};

        // The server's side of the two metadata calls: a list that the
        // test changes, and an answer that repeats what it was asked.
        let listed: Arc<Mutex<Vec<MetadataNetwork>>> = Arc::new(Mutex::new(Vec::new()));
        let list = listed.clone();
        let metadata = axum::Router::new()
            .route(
                "/metadata/networks/{node}",
                axum::routing::get(move || {
                    let list = list.clone();
                    async move { axum::Json(list.lock().unwrap().clone()) }
                }),
            )
            .route(
                "/metadata/guest",
                axum::routing::post(|axum::Json(q): axum::Json<MetadataQuery>| async move {
                    axum::Json(MetadataAnswer {
                        status: 200,
                        body: format!("{} {} {}\n", q.node_name, q.network, q.path).into_bytes(),
                    })
                }),
            );
        let queue = pistack_rpc::CommandQueue::new(pistack_rpc::QueueConfig {
            command_timeout: Duration::from_secs(10),
            capacity: 8,
            heartbeat_interval: Duration::from_secs(1),
            ovn: None,
        });
        let rpc = pistack_rpc::router(queue.clone()).merge(metadata);
        let app = axum::Router::new().nest("/rpc", rpc);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/rpc", listener.local_addr().unwrap());
        let shutdown = CancellationToken::new();
        let stop = shutdown.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(async move { stop.cancelled().await })
                .await
                .unwrap();
        });

        let switch = FakeSwitch::new();
        switch.controller(Some("connected"));
        let host = Arc::new(FakeHost::default());
        let listen = Arc::new(FakeListen::default());
        let meta = Arc::new(MetaNets::with(host.clone(), listen.clone(), true));
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::new());
        // A host with Open vSwitch watches OVN from the start.
        let facts = HostFacts {
            node_name: "node-a".into(),
            resources: Resources::default(),
            capabilities: vec!["ovs".into()],
        };
        let agent = tokio::spawn(run_agent_on(
            AgentClient::new(base, "node-a").unwrap(),
            Runtimes::all(rt),
            facts,
            Arc::new(OvnHost::with(switch.clone(), tools(false))),
            meta,
            Some("10.66.66.5".into()),
            shutdown.clone(),
        ));
        let until = |want: Vec<String>| {
            let host = host.clone();
            async move {
                tokio::time::timeout(Duration::from_secs(10), async {
                    while host.namespaces() != want {
                        tokio::time::sleep(Duration::from_millis(50)).await;
                    }
                })
                .await
                .expect("the agent follows the server's list");
            }
        };

        // No guest on an OVN network here: nothing is built.
        tokio::time::sleep(Duration::from_millis(300)).await;
        assert!(host.namespaces().is_empty());

        // The first guest of a network arrives.
        *listed.lock().unwrap() = vec![net(NET_A, PORT_A, &["10.1.0.0/24"])];
        until(vec![PORT_A.to_owned()]).await;
        let mut reply = String::new();
        for _ in 0..100 {
            // The listener opens a moment after the namespace exists.
            if listen.bound(PORT_A) {
                reply = get(listen.addr(PORT_A), "/latest/meta-data/hostname").await;
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        assert!(reply.starts_with("HTTP/1.1 200 OK\r\n"), "{reply}");
        assert!(
            reply.ends_with(&format!("node-a {NET_A} /latest/meta-data/hostname\n")),
            "{reply}"
        );

        // The last one leaves.
        listed.lock().unwrap().clear();
        until(Vec::new()).await;

        shutdown.cancel();
        agent.await.unwrap().unwrap();
    }

    async fn queue_exec(
        queue: &pistack_rpc::CommandQueue,
        agent: &Agent,
        payload: serde_json::Value,
    ) -> Result<serde_json::Value, ApiError> {
        let ticket = queue.enqueue("node-a", CommandKind::Exec, payload).unwrap();
        let cmd = queue
            .next("node-a", Duration::from_secs(2), 1)
            .await
            .into_iter()
            .next()
            .expect("exec command");
        assert_eq!(cmd.kind, CommandKind::Exec);
        let id = cmd.id;
        let result = match agent.execute(cmd).await {
            Ok(body) => CommandResult::ok(id).with_payload(body),
            Err(e) => CommandResult::from_api_error(id, &e),
        };
        queue.ack("node-a", result).unwrap();
        ticket.wait().await.unwrap().into_api_result()
    }

    fn attach_req(path: &str, target: Option<DiskTarget>) -> VolumeAttach {
        VolumeAttach {
            machine: Id::new(pistack_types::kind::MACHINE),
            volume: Id::new(pistack_types::kind::VOLUME),
            path: path.to_owned(),
            device: Some("vdb".into()),
            read_only: false,
            fs_type: String::new(),
            target,
        }
    }

    #[test]
    fn host_path_must_exist_before_it_is_confirmed() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("disk.raw");
        std::fs::write(&file, b"volume").unwrap();
        let file = file.to_str().unwrap();

        let local = attach_req(file, None);
        let confirmed = decide_host_path(&local, None, None).unwrap();
        assert_eq!(confirmed.as_deref(), Some(file));

        // A device node is a path too. /dev/null is a character device.
        let node = attach_req("/dev/null", None);
        assert_eq!(
            decide_host_path(&node, None, None).unwrap().as_deref(),
            Some("/dev/null")
        );

        let missing = dir.path().join("missing");
        let missing = missing.to_str().unwrap();
        let err = decide_host_path(&attach_req(missing, None), None, None).unwrap_err();
        assert!(err.message.contains("does not exist"), "{err}");

        let err = decide_host_path(&attach_req(dir.path().to_str().unwrap(), None), None, None)
            .unwrap_err();
        assert!(err.message.contains("directory"), "{err}");

        let err = decide_host_path(&attach_req("rel", None), None, None).unwrap_err();
        assert!(err.message.contains("absolute"), "{err}");
        let err = host_path_ready("/tmp/../etc").unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);

        let rbd = attach_req(
            "",
            Some(DiskTarget::Rbd {
                pool: "volumes".into(),
                image: "vol-1".into(),
                monitors: vec!["10.1.0.5:6789".into()],
                user: "pistack".into(),
                key_secret: None,
            }),
        );
        assert!(decide_host_path(&rbd, None, None).unwrap().is_none());
        assert_eq!(
            decide_host_path(&rbd, Some(file), None).unwrap().as_deref(),
            Some(file)
        );
        assert_eq!(
            decide_host_path(&rbd, None, Some("/dev/null"))
                .unwrap()
                .as_deref(),
            Some("/dev/null")
        );
        let err = decide_host_path(&rbd, Some(missing), None).unwrap_err();
        assert!(err.message.contains("does not exist"), "{err}");
        let err = decide_host_path(&rbd, Some(""), None).unwrap_err();
        assert!(err.message.contains("missing"), "{err}");

        let state = dir.path().join("youki");
        let machine = Id::new(pistack_types::kind::MACHINE);
        let volume = Id::new(pistack_types::kind::VOLUME);
        assert!(recorded_disk_path(&state, &machine, &volume).is_none());
        let bundle = state.join(machine.to_string());
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(
            bundle.join("disks.json"),
            serde_json::json!([{
                "volume": volume.to_string(),
                "path": file,
                "device": "vdb"
            }])
            .to_string(),
        )
        .unwrap();
        assert_eq!(
            recorded_disk_path(&state, &machine, &volume).as_deref(),
            Some(file)
        );
    }

    #[tokio::test]
    async fn missing_local_path_is_refused_before_the_runtime_attaches() {
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::MockRuntime::default());
        let missing = attach_req("/var/lib/pistack/missing-volume-path", None);
        let err = confirm_volume_attach(&Runtimes::all(rt), &missing)
            .await
            .unwrap_err();
        assert!(err.message.contains("does not exist"), "{err}");
        assert!(!err.message.contains("does not attach"), "{err}");
    }

    struct SavedEnv {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl SavedEnv {
        fn capture(keys: &[&'static str]) -> Self {
            Self {
                saved: keys
                    .iter()
                    .map(|key| (*key, std::env::var_os(key)))
                    .collect(),
            }
        }
    }

    impl Drop for SavedEnv {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn write_exe(path: &std::path::Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn shell_quote(path: &std::path::Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn tool_script(log: &std::path::Path, name: &str, extra: &str) -> String {
        let quoted = shell_quote(log);
        format!(
            "#!/bin/sh\nprintf '%s\\n' '{name}' \"$@\" >> {quoted}\nprintf '\\n' >> {quoted}\n{extra}\n",
            name = name,
            quoted = quoted,
            extra = extra,
        )
    }

    struct VolfsTools {
        log: std::path::PathBuf,
        losetup: std::path::PathBuf,
        blkid: std::path::PathBuf,
        mkfs: std::path::PathBuf,
        mount: std::path::PathBuf,
        umount: std::path::PathBuf,
        empty: std::path::PathBuf,
        mount_fail: std::path::PathBuf,
    }

    fn install_volfs_tools(dir: &std::path::Path) -> VolfsTools {
        let log = dir.join("volfs.log");
        let empty = dir.join("blkid-empty");
        let mount_fail = dir.join("mount-fail");
        let losetup = dir.join("losetup");
        let blkid = dir.join("blkid");
        let mkfs = dir.join("mkfs.ext4");
        let mount = dir.join("mount");
        let umount = dir.join("umount");
        write_exe(
            &losetup,
            &tool_script(
                &log,
                "losetup",
                "if [ \"$1\" = \"--find\" ]; then printf '%s\\n' /dev/loop7; exit 0; fi\nexit 0",
            ),
        );
        write_exe(
            &blkid,
            &tool_script(
                &log,
                "blkid",
                &format!(
                    "if [ -f {} ]; then exit 2; fi\nprintf '%s\\n' ext4\nexit 0",
                    shell_quote(&empty)
                ),
            ),
        );
        write_exe(&mkfs, &tool_script(&log, "mkfs.ext4", "exit 0"));
        write_exe(
            &mount,
            &tool_script(
                &log,
                "mount",
                &format!(
                    "if [ -f {} ]; then echo 'mount failed: no such device' >&2; exit 1; fi\nexit 0",
                    shell_quote(&mount_fail)
                ),
            ),
        );
        write_exe(&umount, &tool_script(&log, "umount", "exit 0"));
        VolfsTools {
            log,
            losetup,
            blkid,
            mkfs,
            mount,
            umount,
            empty,
            mount_fail,
        }
    }

    fn pod_bundle(state: &std::path::Path, machine: &Id) {
        let bundle = state.join(machine.to_string());
        std::fs::create_dir_all(&bundle).unwrap();
        std::fs::write(bundle.join("config.json"), b"{}").unwrap();
    }

    fn command_names(log: &str) -> Vec<String> {
        let mut names = Vec::new();
        let mut at_start = true;
        for line in log.lines() {
            if line.is_empty() {
                at_start = true;
                continue;
            }
            if at_start {
                names.push(line.to_owned());
                at_start = false;
            }
        }
        names
    }

    #[tokio::test]
    async fn pod_attach_records_the_mount_directory() {
        let _youki = crate::runtime::youki::youki_env_lock().lock().await;
        let _volfs = crate::runtime::volfs::env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install_volfs_tools(tmp.path());
        let data = tmp.path().join("data");
        let state = tmp.path().join("youki");
        std::fs::create_dir_all(&data).unwrap();
        let _saved = SavedEnv::capture(&[
            "PISTACK_YOUKI_STATE",
            "PISTACK_LOSETUP",
            "PISTACK_BLKID",
            "PISTACK_MKFS",
            "PISTACK_MOUNT",
            "PISTACK_UMOUNT",
            "PISTACK_VOLFS_DATA_DIR",
            "PISTACK_VOLFS_SUDO",
            "PISTACK_VOLFS_TEST_EUID",
        ]);
        std::env::set_var("PISTACK_YOUKI_STATE", &state);
        std::env::set_var("PISTACK_LOSETUP", &tools.losetup);
        std::env::set_var("PISTACK_BLKID", &tools.blkid);
        std::env::set_var("PISTACK_MKFS", &tools.mkfs);
        std::env::set_var("PISTACK_MOUNT", &tools.mount);
        std::env::set_var("PISTACK_UMOUNT", &tools.umount);
        std::env::set_var("PISTACK_VOLFS_DATA_DIR", &data);
        std::env::remove_var("PISTACK_VOLFS_SUDO");
        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
        std::fs::write(&tools.empty, b"1").unwrap();

        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"volume").unwrap();
        let req = attach_req(file.to_str().unwrap(), None);
        pod_bundle(&state, &req.machine);
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::YoukiRuntime);
        let mut rts = Runtimes::new();
        rts.insert(MachineKind::Container, rt);

        let attached = confirm_volume_attach(&rts, &req).await.unwrap();
        let mounted = data
            .join("slab")
            .join("mounts")
            .join(req.volume.to_string());
        assert_eq!(attached.host_path.as_deref(), mounted.to_str());
        assert!(mounted.is_dir());
        assert_eq!(
            command_names(&std::fs::read_to_string(&tools.log).unwrap()),
            vec![
                "losetup".to_owned(),
                "blkid".to_owned(),
                "mkfs.ext4".to_owned(),
                "mount".to_owned()
            ]
        );
    }

    #[tokio::test]
    async fn a_start_request_is_pointed_at_a_file_on_this_host() {
        let dir = tempfile::tempdir().unwrap();
        let digest = "ab".repeat(32);
        let source = pistack_rpc::ImageSource {
            id: "img-1".into(),
            sha256: digest.clone(),
            size_bytes: None,
            format: "qcow2".into(),
        };
        // The server is not reachable: any download attempt is an error.
        let agent = |rt: crate::runtime::SharedRuntime| Agent {
            client: AgentClient::new("http://127.0.0.1:1", "n1").unwrap(),
            runtimes: Runtimes::all(rt),
            facts: facts::gather(),
            probes: Arc::new(ProbeBook::new()),
            images: ImageCache::new(dir.path()),
            ovn: Arc::new(OvnHost::system()),
            ovn_watch: Arc::new(AtomicBool::new(false)),
            meta: Arc::new(MetaNets::system()),
            underlay: None,
            done: Arc::new(Completed::in_memory()),
            unsent: Mutex::new(Vec::new()),
        };
        let request = |image: &str, source: Option<pistack_rpc::ImageSource>| MachineRequest {
            id: Id::new(pistack_types::kind::MACHINE),
            name: "web".into(),
            kind: MachineKind::Vm,
            image: image.into(),
            image_source: source,
            resources: Default::default(),
            networks: vec![],
            nics: vec![],
            volumes: vec![],
            user_data: None,
            ssh_keys: vec![],
            overlay: None,
            bmc_address: None,
            bmc_user: None,
        };
        let libvirt = || -> crate::runtime::SharedRuntime { Arc::new(HoldRuntime) };

        // The file the server named is here too (one host, or shared disk).
        let here = dir.path().join("here.qcow2");
        std::fs::write(&here, b"disk").unwrap();
        let here = here.to_string_lossy().into_owned();
        let mut req = request(&here, Some(source.clone()));
        agent(libvirt()).localize_image(&mut req).await.unwrap();
        assert_eq!(req.image, here);

        // The server's own blob has no extension: the guest gets a name
        // the hypervisor's confinement accepts.
        let blob = dir.path().join("blob");
        std::fs::write(&blob, b"disk").unwrap();
        let mut req = request(&blob.to_string_lossy(), Some(source.clone()));
        agent(libvirt()).localize_image(&mut req).await.unwrap();
        let adopted = dir.path().join("images").join(format!("{digest}.qcow2"));
        assert_eq!(req.image, adopted.to_string_lossy());
        assert_eq!(std::fs::read(&adopted).unwrap(), b"disk");
        std::fs::remove_file(&adopted).unwrap();

        // No catalog image named: the runtime resolves the name as before.
        let mut req = request("ubuntu-24.04", None);
        agent(libvirt()).localize_image(&mut req).await.unwrap();
        assert_eq!(req.image, "ubuntu-24.04");

        // The mock runtime boots nothing, so it downloads nothing.
        let mut req = request("/srv/images/elsewhere.qcow2", Some(source.clone()));
        agent(Arc::new(crate::runtime::MockRuntime::new()))
            .localize_image(&mut req)
            .await
            .unwrap();
        assert_eq!(req.image, "/srv/images/elsewhere.qcow2");

        // Not here and not cached: the agent asks the server.
        let mut req = request("/srv/images/elsewhere.qcow2", Some(source.clone()));
        assert!(agent(libvirt()).localize_image(&mut req).await.is_err());

        // Already downloaded: the request gets the local copy.
        let cached = dir.path().join("images").join(format!("{digest}.qcow2"));
        std::fs::create_dir_all(cached.parent().unwrap()).unwrap();
        std::fs::write(&cached, b"disk").unwrap();
        let mut req = request("/srv/images/elsewhere.qcow2", Some(source));
        agent(libvirt()).localize_image(&mut req).await.unwrap();
        assert_eq!(req.image, cached.to_string_lossy());
    }

    /// A runtime that accepts the disk and does not write `disks.json`.
    struct HoldRuntime;

    #[async_trait]
    impl MachineRuntime for HoldRuntime {
        fn name(&self) -> &'static str {
            "libvirt"
        }

        async fn start(&self, _req: &MachineRequest) -> ApiResult<RunningMachine> {
            Err(ApiError::not_implemented("unused"))
        }

        async fn stop(&self, _id: &Id) -> ApiResult<()> {
            Ok(())
        }

        async fn delete(&self, _id: &Id) -> ApiResult<()> {
            Ok(())
        }

        async fn list(&self) -> ApiResult<Vec<RunningMachine>> {
            Ok(Vec::new())
        }

        async fn attach_volume(&self, req: &VolumeAttach) -> ApiResult<String> {
            Ok(req.device.clone().unwrap_or_else(|| "vdb".into()))
        }

        async fn detach_volume(&self, _req: &VolumeDetach) -> ApiResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn vm_attach_keeps_the_file_path() {
        let _volfs = crate::runtime::volfs::env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let log = tmp.path().join("losetup.log");
        let losetup = tmp.path().join("losetup");
        write_exe(
            &losetup,
            &tool_script(&log, "losetup", "echo 'losetup should not run' >&2\nexit 1"),
        );
        let data = tmp.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        let _saved = SavedEnv::capture(&[
            "PISTACK_LOSETUP",
            "PISTACK_VOLFS_DATA_DIR",
            "PISTACK_VOLFS_SUDO",
            "PISTACK_VOLFS_TEST_EUID",
        ]);
        std::env::set_var("PISTACK_LOSETUP", &losetup);
        std::env::set_var("PISTACK_VOLFS_DATA_DIR", &data);
        std::env::remove_var("PISTACK_VOLFS_SUDO");
        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");

        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"volume").unwrap();
        let req = attach_req(file.to_str().unwrap(), None);
        let rt: crate::runtime::SharedRuntime = Arc::new(HoldRuntime);
        let mut rts = Runtimes::new();
        rts.insert(MachineKind::Vm, rt);

        let attached = confirm_volume_attach(&rts, &req).await.unwrap();
        assert_eq!(attached.host_path.as_deref(), file.to_str());
        assert!(!log.exists());
        let mounted = data
            .join("slab")
            .join("mounts")
            .join(req.volume.to_string());
        assert!(!mounted.exists());
    }

    #[tokio::test]
    async fn pod_rbd_mount_failure_unmaps_the_device() {
        let _youki = crate::runtime::youki::youki_env_lock().lock().await;
        let _volfs = crate::runtime::volfs::env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install_volfs_tools(tmp.path());
        let data = tmp.path().join("data");
        let state = tmp.path().join("youki");
        let dev_root = tmp.path().join("dev");
        std::fs::create_dir_all(&data).unwrap();
        std::fs::create_dir_all(&dev_root).unwrap();
        std::fs::write(&tools.empty, b"1").unwrap();
        std::fs::write(&tools.mount_fail, b"1").unwrap();

        let rbd_log = tmp.path().join("rbd.log");
        let rbd_state = tmp.path().join("rbd-state");
        let rbd = tmp.path().join("rbd");
        write_exe(
            &rbd,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {log}\nprintf '\\n' >> {log}\n\
                 if [ \"$1\" = device ] && [ \"$2\" = list ]; then\n\
                   if [ -s {state} ]; then cat {state}; else printf '%s\\n' '[]'; fi\n\
                   exit 0\nfi\n\
                 if [ \"$1\" = device ] && [ \"$2\" = map ]; then\n\
                   printf '%s\\n' '[{{\"id\":\"0\",\"pool\":\"volumes\",\"namespace\":\"\",\"name\":\"img-1\",\"snap\":\"-\",\"device\":\"/dev/rbd0\"}}]' > {state}\n\
                   printf '%s\\n' /dev/rbd0\nexit 0\nfi\n\
                 if [ \"$1\" = device ] && [ \"$2\" = unmap ]; then\n\
                   printf '%s\\n' '[]' > {state}\nexit 0\nfi\nexit 1\n",
                log = shell_quote(&rbd_log),
                state = shell_quote(&rbd_state),
            ),
        );

        let _saved = SavedEnv::capture(&[
            "PISTACK_YOUKI_STATE",
            "PISTACK_LOSETUP",
            "PISTACK_BLKID",
            "PISTACK_MKFS",
            "PISTACK_MOUNT",
            "PISTACK_UMOUNT",
            "PISTACK_VOLFS_DATA_DIR",
            "PISTACK_VOLFS_SUDO",
            "PISTACK_VOLFS_TEST_EUID",
            "PISTACK_RBD",
            "PISTACK_RBD_SUDO",
            "PISTACK_RBD_SUDO_BIN",
            "PISTACK_RBD_TEST_EUID",
            "PISTACK_RBD_DEV_ROOT",
        ]);
        std::env::set_var("PISTACK_YOUKI_STATE", &state);
        std::env::set_var("PISTACK_LOSETUP", &tools.losetup);
        std::env::set_var("PISTACK_BLKID", &tools.blkid);
        std::env::set_var("PISTACK_MKFS", &tools.mkfs);
        std::env::set_var("PISTACK_MOUNT", &tools.mount);
        std::env::set_var("PISTACK_UMOUNT", &tools.umount);
        std::env::set_var("PISTACK_VOLFS_DATA_DIR", &data);
        std::env::remove_var("PISTACK_VOLFS_SUDO");
        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
        std::env::set_var("PISTACK_RBD", &rbd);
        std::env::remove_var("PISTACK_RBD_SUDO");
        std::env::remove_var("PISTACK_RBD_SUDO_BIN");
        std::env::set_var("PISTACK_RBD_TEST_EUID", "0");
        std::env::set_var("PISTACK_RBD_DEV_ROOT", &dev_root);

        let req = attach_req(
            "",
            Some(DiskTarget::Rbd {
                pool: "volumes".into(),
                image: "img-1".into(),
                monitors: vec!["10.1.0.5:6789".into()],
                user: "pistack".into(),
                key_secret: None,
            }),
        );
        pod_bundle(&state, &req.machine);
        let rt: crate::runtime::SharedRuntime = Arc::new(crate::runtime::YoukiRuntime);
        let mut rts = Runtimes::new();
        rts.insert(MachineKind::Container, rt);

        let err = confirm_volume_attach(&rts, &req).await.unwrap_err();
        assert!(err.message.contains("mount"), "{err}");
        let volfs_log = std::fs::read_to_string(&tools.log).unwrap();
        let names = command_names(&volfs_log);
        assert!(names.contains(&"blkid".to_owned()), "{volfs_log}");
        assert!(names.contains(&"mkfs.ext4".to_owned()), "{volfs_log}");
        assert!(names.contains(&"mount".to_owned()), "{volfs_log}");
        assert!(names.contains(&"umount".to_owned()), "{volfs_log}");
        assert!(!names.contains(&"losetup".to_owned()), "{volfs_log}");
        let rbd_text = std::fs::read_to_string(&rbd_log).unwrap();
        assert!(rbd_text.contains("unmap"), "{rbd_text}");
        let disks = std::fs::read_to_string(state.join(req.machine.to_string()).join("disks.json"))
            .unwrap_or_default();
        assert!(!disks.contains(&req.volume.to_string()), "{disks}");
        let mounted = data
            .join("slab")
            .join("mounts")
            .join(req.volume.to_string());
        assert!(!mounted.exists());
        assert!(!mounted.with_extension("loop").exists());
    }

    fn agent_on(base: &str, rt: crate::runtime::SharedRuntime, done: Completed) -> Agent {
        Agent {
            client: AgentClient::new(base, "node-a").unwrap(),
            runtimes: Runtimes::all(rt),
            facts: facts::gather(),
            probes: Arc::new(ProbeBook::new()),
            images: ImageCache::new(std::path::Path::new("/nonexistent")),
            ovn: Arc::new(OvnHost::system()),
            ovn_watch: Arc::new(AtomicBool::new(false)),
            meta: Arc::new(MetaNets::system()),
            underlay: None,
            done: Arc::new(done),
            unsent: Mutex::new(Vec::new()),
        }
    }

    fn command(kind: CommandKind, machine: &Id, remember: bool) -> Command {
        Command {
            id: uuid::Uuid::now_v7(),
            kind,
            payload: serde_json::json!({
                "id": machine.to_string(),
                "name": "web",
                "image": "ubuntu-24.04"
            }),
            issued_at: Utc::now(),
            remember,
        }
    }

    #[tokio::test]
    async fn a_remembered_command_delivered_again_is_not_run_again() {
        use crate::runtime::{MachineRuntime as _, PowerState};

        let dir = tempfile::tempdir().unwrap();
        let record = dir.path().join(crate::done::FILE);
        let mock = Arc::new(crate::runtime::MockRuntime::new());
        let machine = Id::new(pistack_types::kind::MACHINE);
        let power = || async { mock.list().await.unwrap()[0].power };
        let start = || command(CommandKind::StartMachine, &machine, false);
        let stop = command(CommandKind::StopMachine, &machine, true);

        let agent = agent_on("http://127.0.0.1:1", mock.clone(), Completed::open(&record));
        let first = agent.run_all(vec![start(), stop.clone()]).await;
        assert!(first.iter().all(|r| r.ok));
        assert_eq!(power().await, PowerState::Off);

        // Started again, and then the old stop arrives a second time, as it
        // does when a new leader issues what the old one had outstanding.
        let again = agent.run_all(vec![start(), stop.clone()]).await;
        assert_eq!(again[1], first[1]);
        assert_eq!(power().await, PowerState::On);

        // The agent restarts and still knows the id.
        let agent = agent_on("http://127.0.0.1:1", mock.clone(), Completed::open(&record));
        let after = agent.run_all(vec![stop.clone()]).await;
        assert_eq!(after[0], first[1]);
        assert_eq!(power().await, PowerState::On);

        // A command that is not marked runs every time it arrives.
        let plain = Command {
            remember: false,
            ..stop
        };
        assert!(agent.run_all(vec![plain]).await[0].ok);
        assert_eq!(power().await, PowerState::Off);
    }

    #[tokio::test]
    async fn a_result_the_old_server_did_not_take_goes_to_the_next_one() {
        let queue = pistack_rpc::CommandQueue::default();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let live = format!("http://{}", listener.local_addr().unwrap());
        let app = axum::Router::new().nest("/rpc", pistack_rpc::router(queue.clone()));
        let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        // The agent's server is gone; it has heard of the other one.
        let mock = Arc::new(crate::runtime::MockRuntime::new());
        let agent = agent_on("http://127.0.0.1:1/rpc", mock, Completed::in_memory());
        agent.client.learn_servers([live.as_str()]);

        let machine = Id::new(pistack_types::kind::MACHINE);
        let payload = serde_json::json!({"id": machine.to_string()});
        let kept = queue
            .enqueue_keyed(
                "node-a",
                "machine/m",
                CommandKind::StopMachine,
                payload.clone(),
            )
            .unwrap();
        let dropped = queue
            .enqueue("node-a", CommandKind::StopMachine, payload)
            .unwrap();
        let cmds = queue.next("node-a", Duration::ZERO, 16).await;
        assert_eq!(cmds.len(), 2);

        agent.execute_all(cmds).await;
        // Only the remembered result waits to be sent again.
        assert_eq!(agent.unsent.lock().unwrap().len(), 1);
        assert_eq!(queue.pending_count(), 2);

        agent.resend().await;
        assert!(agent.unsent.lock().unwrap().is_empty());
        let result = tokio::time::timeout(Duration::from_secs(5), kept.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(!result.ok, "the mock holds no such machine");
        assert_eq!(queue.pending_count(), 1);
        drop(dropped);
        server.abort();
    }
}
