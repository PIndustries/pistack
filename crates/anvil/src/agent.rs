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
//! any command the server had just marked delivered. Liveness and readiness
//! probes run on their own tick and ride back on the heartbeat.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::Utc;
use keep::Pki;
use pistack_rpc::proto::{
    AgentHello, Command, CommandKind, CommandResult, ExecRequest, Heartbeat, LogText, MachineKind,
    MachineRef, MachineReport, MachineRequest, MigrateRequest, PortPlug, VolumeAttach,
    VolumeAttached, VolumeDetach, MAX_LONG_POLL_SECS,
};
use pistack_rpc::{AgentClient, RpcError, RPC_INSECURE_ENV};
use pistack_types::{ApiError, Id, Phase, Resources};
use tokio_util::sync::CancellationToken;

use crate::facts::{self, HostFacts};
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

/// Build the agent client. Prefer a persisted client certificate; otherwise
/// enroll with the join token when present. Plain HTTP without a certificate
/// only works when the control plane sets `PISTACK_RPC_INSECURE=1`.
async fn connect_and_enroll(opts: &AgentOpts, node: &str) -> anyhow::Result<AgentClient> {
    let base = join_url(&opts.join);
    let token = normalize_join_token(opts.token.clone());
    let cert_dir = opts.data_dir.join("agent");

    if let Some(pki) = load_agent_pki(&cert_dir)? {
        tracing::info!("using persisted agent client certificate");
        let mut client = AgentClient::with_mtls(&base, node, &pki.ca_pem, &pki)?;
        if let Some(t) = token {
            client = client.with_token(t);
        }
        return Ok(client);
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
                return Ok(AgentClient::with_mtls(&base, node, &pki.ca_pem, &pki)?);
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
    let agent = Agent {
        client,
        runtimes,
        facts,
        probes: Arc::new(ProbeBook::new()),
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
        let probes = tokio::spawn(self.clone_for_probes().run(session.clone()));

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
        let _ = probes.await;
        end
    }

    fn clone_for_heartbeat(&self) -> HeartbeatTask {
        HeartbeatTask {
            client: self.client.clone(),
            runtimes: self.runtimes.clone(),
            resources: self.facts.resources,
            probes: self.probes.clone(),
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
            CommandKind::PlugPort => {
                let req: PortPlug = payload(c.payload)?;
                tracing::info!(machine = %req.machine, "anvil: plug port");
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

/// Cloud Hypervisor is the VM runtime only when this host selected it.
/// libvirt keeps port plug on every other host, including the mock.
fn cloud_vm(runtimes: &Runtimes) -> bool {
    runtimes
        .get(MachineKind::Vm)
        .is_some_and(|rt| rt.name() == "cloud-hypervisor")
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
    match std::env::var_os("PISTACK_YOUKI_STATE") {
        Some(v) if !v.is_empty() => std::path::PathBuf::from(v),
        _ => std::path::PathBuf::from("/var/lib/pistack/youki"),
    }
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
    let mut candidates = Vec::new();
    let bundle = youki_state.join(&name);
    candidates.push((bundle.join("log"), bundle.clone()));
    candidates.push((bundle.join("console.log"), bundle));
    let domain = format!("pistack-{name}");
    let vm = libvirt_root.join("machines").join(domain);
    candidates.push((vm.join("console.log"), vm));

    let mut empty = None;
    for (path, jail) in candidates {
        if let Some(text) = read_jailed_log(&path, &jail).await? {
            if !text.is_empty() {
                return Ok(scrub_log(&text));
            }
            if empty.is_none() {
                empty = Some(text);
            }
        }
    }
    if let Some(text) = empty {
        return Ok(scrub_log(&text));
    }
    Err(ApiError::not_found(format!("no log for machine {id}")))
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

struct HeartbeatTask {
    client: AgentClient,
    runtimes: Runtimes,
    resources: Resources,
    probes: Arc<ProbeBook>,
}

/// Runs due probes about once a second. Adoption from the runtime (so a
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
            let outcome = probe::execute(&check).await;
            if !outcome.success {
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
            let hb = Heartbeat {
                node_name: self.client.node_name().to_owned(),
                resources_free: self.resources,
                machines: rows,
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
        probes: Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::MachineRuntime;

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
        };
        let bad = agent
            .execute(Command {
                id: uuid::Uuid::now_v7(),
                kind: CommandKind::ReadLogs,
                payload: serde_json::json!({}),
                issued_at: Utc::now(),
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
        };
        let queue = pistack_rpc::CommandQueue::new(pistack_rpc::QueueConfig {
            command_timeout: Duration::from_secs(5),
            capacity: 4,
            heartbeat_interval: Duration::from_secs(30),
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
}
