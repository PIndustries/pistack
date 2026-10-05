// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Liveness and readiness checks run on the host that holds the pod.
//!
//! The agent reads probe definitions from the sandbox document (or from
//! the runtime, after a restart). `http_get` and `tcp_socket` connect to
//! the pod's sandbox address. `exec` runs its argv in the named container
//! through the runtime that holds the pod. youki uses the same exec helper
//! as `pi pod exec`, so `PISTACK_YOUKI_SUDO` and the rootless fallback apply
//! unchanged. Exit code 0 is a pass. A non-zero exit or a timeout is a
//! failure. Any other runtime reports the probe unsupported. An unsupported
//! result does not count as a failed check and is not a pass.
//!
//! Results are consecutive streaks reported on the ordinary machine
//! heartbeat. Period, timeout, and the success and failure thresholds are
//! the same for every handler. Captured command output is capped and
//! scrubbed before it is stored or logged. Probe env is not passed and is
//! not logged.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use pistack_rpc::proto::{ExecOutput, ProbeReport, PROBE_LIVENESS, PROBE_READINESS};
use pistack_types::{error::ErrorCode, ApiError, Id};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::future::Future;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

/// Most bytes of stdout and of stderr kept from one command probe.
/// The stored message keeps a shorter scrubbed excerpt.
pub(crate) const PROBE_EXEC_OUTPUT_CAP: usize = 1024;

const PROBE_MESSAGE_CAP: usize = 160;

const DEFAULT_PERIOD: u32 = 10;
const DEFAULT_TIMEOUT: u32 = 1;
const DEFAULT_SUCCESS: u32 = 1;
const DEFAULT_FAILURE: u32 = 3;

fn nonzero(value: u32, default: u32) -> u32 {
    if value == 0 {
        default
    } else {
        value
    }
}

fn default_period() -> u32 {
    DEFAULT_PERIOD
}
fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT
}
fn default_success() -> u32 {
    DEFAULT_SUCCESS
}
fn default_failure() -> u32 {
    DEFAULT_FAILURE
}
fn default_path() -> String {
    "/".into()
}

/// Probe fields copied from a container spec. The JSON shape matches the
/// pod spec so a sandbox document deserialises without a translation step.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProbeSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub http_get: Option<HttpGetSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tcp_socket: Option<TcpSocketSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exec: Option<ExecSpec>,
    #[serde(default)]
    pub initial_delay_seconds: u32,
    #[serde(default = "default_period")]
    pub period_seconds: u32,
    #[serde(default = "default_timeout")]
    pub timeout_seconds: u32,
    #[serde(default = "default_success")]
    pub success_threshold: u32,
    #[serde(default = "default_failure")]
    pub failure_threshold: u32,
}

impl ProbeSpec {
    fn period(&self) -> u32 {
        nonzero(self.period_seconds, DEFAULT_PERIOD)
    }
    fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(nonzero(self.timeout_seconds, DEFAULT_TIMEOUT)))
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpGetSpec {
    #[serde(default = "default_path")]
    pub path: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<ProbeScheme>,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "UPPERCASE")]
pub enum ProbeScheme {
    #[default]
    #[serde(alias = "http")]
    Http,
    #[serde(alias = "https")]
    Https,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct TcpSocketSpec {
    pub port: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecSpec {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

/// Probes for one container, persisted by a runtime so a restarted agent
/// can keep checking without the original sandbox document.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerProbes {
    pub container: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness: Option<ProbeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<ProbeSpec>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ProbeKind {
    Liveness,
    Readiness,
}

impl ProbeKind {
    fn as_str(self) -> &'static str {
        match self {
            ProbeKind::Liveness => PROBE_LIVENESS,
            ProbeKind::Readiness => PROBE_READINESS,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ProbeAction {
    Http {
        path: String,
        port: u16,
        host: Option<String>,
        https: bool,
    },
    Tcp {
        port: u16,
    },
    Exec {
        command: Vec<String>,
    },
    Missing,
}

impl ProbeSpec {
    fn action(&self) -> ProbeAction {
        if let Some(http) = &self.http_get {
            return ProbeAction::Http {
                path: http.path.clone(),
                port: http.port,
                host: http.host.clone(),
                https: matches!(http.scheme, Some(ProbeScheme::Https)),
            };
        }
        if let Some(tcp) = &self.tcp_socket {
            return ProbeAction::Tcp { port: tcp.port };
        }
        if let Some(exec) = &self.exec {
            return ProbeAction::Exec {
                command: exec.command.clone(),
            };
        }
        ProbeAction::Missing
    }
}

/// A machine the probe loop can reach right now.
#[derive(Clone, Debug)]
pub struct ProbeTarget {
    pub id: Id,
    pub power_on: bool,
    pub address: Option<String>,
}

#[derive(Clone, Debug)]
pub struct DueCheck {
    pub machine: Id,
    pub generation: u64,
    pub container: String,
    pub kind: String,
    pub action: ProbeAction,
    pub timeout: Duration,
    pub address: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Outcome {
    pub success: bool,
    pub unsupported: bool,
    pub message: String,
}

impl Outcome {
    fn ok() -> Self {
        Self {
            success: true,
            unsupported: false,
            message: String::new(),
        }
    }
    fn fail(message: impl Into<String>) -> Self {
        Self {
            success: false,
            unsupported: false,
            message: message.into(),
        }
    }
    fn unsupported(message: impl Into<String>) -> Self {
        Self {
            success: false,
            unsupported: true,
            message: message.into(),
        }
    }
}

#[derive(Clone, Debug)]
struct Track {
    container: String,
    kind: ProbeKind,
    spec: ProbeSpec,
    started: DateTime<Utc>,
    last_attempt: Option<DateTime<Utc>>,
    successes: u32,
    failures: u32,
    success: bool,
    unsupported: bool,
    message: String,
}

impl Track {
    fn note(&mut self, outcome: &Outcome) {
        if outcome.unsupported {
            self.successes = 0;
            self.failures = 0;
            self.success = false;
            self.unsupported = true;
            self.message = outcome.message.clone();
            return;
        }
        self.unsupported = false;
        if outcome.success {
            self.successes = self.successes.saturating_add(1);
            self.failures = 0;
            self.success = true;
            self.message.clear();
        } else {
            self.failures = self.failures.saturating_add(1);
            self.successes = 0;
            self.success = false;
            self.message = outcome.message.clone();
        }
    }

    fn due(&self, now: DateTime<Utc>) -> bool {
        let delay = ChronoDuration::seconds(i64::from(self.spec.initial_delay_seconds));
        if now.signed_duration_since(self.started) < delay {
            return false;
        }
        let period = ChronoDuration::seconds(i64::from(self.spec.period()));
        match self.last_attempt {
            None => true,
            Some(at) => now.signed_duration_since(at) >= period,
        }
    }
}

struct MachineProbes {
    generation: u64,
    tracks: Vec<Track>,
}

#[derive(Default)]
struct Book {
    machines: BTreeMap<Id, MachineProbes>,
}

impl Book {
    fn install(&mut self, id: &Id, specs: Vec<ContainerProbes>, now: DateTime<Utc>) {
        let tracks = tracks_from(specs, now);
        if tracks.is_empty() {
            self.machines.remove(id);
            return;
        }
        let generation = self
            .machines
            .get(id)
            .map(|m| m.generation.saturating_add(1))
            .unwrap_or(1);
        self.machines
            .insert(id.clone(), MachineProbes { generation, tracks });
    }

    fn due(&mut self, targets: &[ProbeTarget], now: DateTime<Utc>) -> Vec<DueCheck> {
        let mut out = Vec::new();
        for target in targets {
            if !target.power_on {
                continue;
            }
            let Some(machine) = self.machines.get_mut(&target.id) else {
                continue;
            };
            let generation = machine.generation;
            for track in &mut machine.tracks {
                if !track.due(now) {
                    continue;
                }
                track.last_attempt = Some(now);
                out.push(DueCheck {
                    machine: target.id.clone(),
                    generation,
                    container: track.container.clone(),
                    kind: track.kind.as_str().to_owned(),
                    action: track.spec.action(),
                    timeout: track.spec.timeout(),
                    address: target.address.clone(),
                });
            }
        }
        out
    }

    fn record(&mut self, check: &DueCheck, outcome: &Outcome) {
        let Some(machine) = self.machines.get_mut(&check.machine) else {
            return;
        };
        if machine.generation != check.generation {
            return;
        }
        if let Some(track) = machine
            .tracks
            .iter_mut()
            .find(|t| t.container == check.container && t.kind.as_str() == check.kind)
        {
            track.note(outcome);
        }
    }

    fn reports(&self, id: &Id) -> Vec<ProbeReport> {
        let Some(machine) = self.machines.get(id) else {
            return Vec::new();
        };
        machine
            .tracks
            .iter()
            .map(|t| ProbeReport {
                container: t.container.clone(),
                kind: t.kind.as_str().to_owned(),
                success: t.success,
                successes: t.successes,
                failures: t.failures,
                message: t.message.clone(),
                unsupported: t.unsupported,
            })
            .collect()
    }
}

fn tracks_from(specs: Vec<ContainerProbes>, now: DateTime<Utc>) -> Vec<Track> {
    let mut tracks = Vec::new();
    for spec in specs {
        if let Some(liveness) = spec.liveness {
            tracks.push(new_track(
                spec.container.clone(),
                ProbeKind::Liveness,
                liveness,
                now,
            ));
        }
        if let Some(readiness) = spec.readiness {
            tracks.push(new_track(
                spec.container,
                ProbeKind::Readiness,
                readiness,
                now,
            ));
        }
    }
    tracks
}

fn new_track(container: String, kind: ProbeKind, spec: ProbeSpec, now: DateTime<Utc>) -> Track {
    Track {
        container,
        kind,
        spec,
        started: now,
        last_attempt: None,
        successes: 0,
        failures: 0,
        success: false,
        unsupported: false,
        message: String::new(),
    }
}

/// In-memory probe streaks for every sandbox on this host.
#[derive(Default)]
pub struct ProbeBook {
    inner: Mutex<Book>,
}

impl ProbeBook {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Book> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Replace the streaks for `id`. An empty list forgets the machine.
    /// A later [`Self::record`] from a check issued before this call is
    /// ignored.
    pub fn install(&self, id: &Id, specs: Vec<ContainerProbes>, now: DateTime<Utc>) {
        self.lock().install(id, specs, now);
    }

    pub fn forget(&self, id: &Id) {
        self.lock().machines.remove(id);
    }

    pub fn has_tracks(&self) -> bool {
        self.lock().machines.values().any(|m| !m.tracks.is_empty())
    }

    pub fn contains(&self, id: &Id) -> bool {
        self.lock().machines.contains_key(id)
    }

    /// Drop machines that are no longer on the host.
    pub fn retain(&self, live: &BTreeSet<Id>) {
        self.lock().machines.retain(|id, _| live.contains(id));
    }

    /// Checks whose period has arrived. Marks them attempted so a slow
    /// check does not start again before the next period.
    pub fn due(&self, targets: &[ProbeTarget], now: DateTime<Utc>) -> Vec<DueCheck> {
        self.lock().due(targets, now)
    }

    pub fn record(&self, check: &DueCheck, outcome: &Outcome) {
        self.lock().record(check, outcome);
    }

    pub fn reports(&self, id: &Id) -> Vec<ProbeReport> {
        self.lock().reports(id)
    }
}

/// Probe definitions carried on a pod sandbox document. Anything that is
/// not a sandbox, or that does not parse, yields an empty list.
pub fn probes_in_user_data(raw: Option<&str>) -> Vec<ContainerProbes> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let doc: SandboxProbes = match serde_json::from_str(raw) {
        Ok(doc) => doc,
        Err(_) => return Vec::new(),
    };
    if doc.kind != "PodSandbox" {
        return Vec::new();
    }
    doc.containers
        .into_iter()
        .filter(|c| c.liveness_probe.is_some() || c.readiness_probe.is_some())
        .map(|c| ContainerProbes {
            container: c.name,
            liveness: c.liveness_probe,
            readiness: c.readiness_probe,
        })
        .collect()
}

#[derive(Deserialize)]
struct SandboxProbes {
    #[serde(default)]
    kind: String,
    #[serde(default)]
    containers: Vec<SandboxProbeContainer>,
}

#[derive(Deserialize)]
struct SandboxProbeContainer {
    #[serde(default)]
    name: String,
    #[serde(default)]
    liveness_probe: Option<ProbeSpec>,
    #[serde(default)]
    readiness_probe: Option<ProbeSpec>,
}

/// What a runtime did with one command probe.
///
/// `Absent` means this runtime does not hold the machine. `Unsupported`
/// means it cannot run a command inside a container. Neither is a pass,
/// and neither counts as a failure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CommandProbeReply {
    Absent,
    Unsupported,
    Finished { exit_code: i32, output: String },
    TimedOut,
    Failed { message: String },
}

/// A command probe needs at least one argument, and no argument may contain
/// a NUL byte. youki would reject the same argv; checking it here keeps the
/// probe message stable.
pub(crate) fn validate_exec_argv(argv: &[String]) -> Result<(), &'static str> {
    if argv.is_empty() || argv.iter().any(|arg| arg.is_empty()) {
        return Err("exec command is empty");
    }
    if argv.iter().any(|arg| arg.contains('\0')) {
        return Err("exec command contains a NUL byte");
    }
    Ok(())
}

/// Map a youki exec result onto a command-probe reply. A missing machine is
/// absent, a timeout is a timeout, and `not_implemented` is unsupported.
/// Other errors are failures. The output excerpt is capped by the caller.
pub(crate) fn reply_from_exec(result: Result<ExecOutput, ApiError>) -> CommandProbeReply {
    match result {
        Ok(out) => CommandProbeReply::Finished {
            exit_code: out.exit_code,
            output: combine_output(&out.stdout, &out.stderr),
        },
        Err(err) if err.code == ErrorCode::NotImplemented => CommandProbeReply::Unsupported,
        Err(err) if err.code == ErrorCode::NotFound && err.message.contains("not on this host") => {
            CommandProbeReply::Absent
        }
        Err(err) if err.message.contains("timed out") => CommandProbeReply::TimedOut,
        Err(err) => CommandProbeReply::Failed {
            message: err.message,
        },
    }
}

fn combine_output(stdout: &str, stderr: &str) -> String {
    match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => String::new(),
        (false, true) => stdout.to_owned(),
        (true, false) => stderr.to_owned(),
        (false, false) => format!("{stderr}\n{stdout}"),
    }
}

pub(crate) fn outcome_of_reply(reply: CommandProbeReply) -> Outcome {
    match reply {
        CommandProbeReply::Absent | CommandProbeReply::Unsupported => {
            Outcome::unsupported("exec probes are not supported")
        }
        CommandProbeReply::Finished { exit_code: 0, .. } => Outcome::ok(),
        CommandProbeReply::Finished { exit_code, output } => {
            Outcome::fail(exit_message(exit_code, &output))
        }
        CommandProbeReply::TimedOut => Outcome::fail("timed out"),
        CommandProbeReply::Failed { message } => {
            let message = scrub_probe_text(&message);
            if message.is_empty() {
                Outcome::fail("exec probe failed")
            } else {
                Outcome::fail(message)
            }
        }
    }
}

fn exit_message(exit_code: i32, output: &str) -> String {
    let output = scrub_probe_text(output);
    if output.is_empty() {
        format!("exit {exit_code}")
    } else {
        format!("exit {exit_code}: {output}")
    }
}

/// Run one check when no runtime is available. An exec probe is unsupported,
/// so it cannot become a pass.
#[cfg(test)]
pub async fn execute(check: &DueCheck) -> Outcome {
    run(check, |_, _| async { CommandProbeReply::Unsupported }).await
}

/// Run one check. `exec` calls `exec_probe` with the probe argv and timeout.
/// A probe with no handler is unsupported and does not count as a failure.
pub async fn run<F, Fut>(check: &DueCheck, exec_probe: F) -> Outcome
where
    F: FnOnce(&[String], Duration) -> Fut,
    Fut: Future<Output = CommandProbeReply>,
{
    match &check.action {
        ProbeAction::Exec { command } => {
            if let Err(message) = validate_exec_argv(command) {
                return Outcome::fail(message);
            }
            outcome_of_reply(exec_probe(command, check.timeout).await)
        }
        ProbeAction::Missing => Outcome::unsupported("probe has no handler"),
        ProbeAction::Tcp { port } => tcp_probe(check, *port).await,
        ProbeAction::Http {
            path,
            port,
            host,
            https,
        } => http_probe(check, path, *port, host.as_deref(), *https).await,
    }
}

fn scrub_probe_text(text: &str) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(|line| line.trim_end_matches('\r'))
        .filter(|line| !line.trim().is_empty())
        .collect();
    if lines.iter().copied().any(secret_probe_line) {
        return String::new();
    }
    let flat = lines
        .iter()
        .map(|line| redact_probe_secrets(line.trim()))
        .collect::<Vec<_>>()
        .join(" ");
    let flat = flat.split_whitespace().collect::<Vec<_>>().join(" ");
    flat.chars().take(PROBE_MESSAGE_CAP).collect()
}

fn secret_probe_line(line: &str) -> bool {
    let lower = line.to_ascii_lowercase();
    const MARKERS: &[&str] = &[
        "begin private key",
        "begin openssh",
        "begin rsa private key",
        "begin ec private key",
        "ssh-ed25519",
        "ssh-rsa",
        "authorized_keys",
    ];
    MARKERS.iter().any(|marker| lower.contains(marker))
}

fn redact_probe_secrets(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut rest = line;
    while let Some(rel) = rest.find(['=', ':']) {
        let (head, tail) = rest.split_at(rel);
        let sep = tail.chars().next().unwrap();
        let after = &tail[sep.len_utf8()..];
        out.push_str(head);
        out.push(sep);
        if key_is_secret(head) {
            if head.to_ascii_lowercase().contains("authorization") {
                out.push_str(" ***");
                return out;
            }
            let spaces = after.len() - after.trim_start_matches(' ').len();
            out.push_str(&after[..spaces]);
            out.push_str("***");
            let value = &after[spaces..];
            let skip = value
                .find(|c: char| c.is_whitespace())
                .unwrap_or(value.len());
            rest = &value[skip..];
        } else {
            rest = after;
        }
    }
    out.push_str(rest);
    out
}

fn key_is_secret(head: &str) -> bool {
    let token = head
        .rsplit(|c: char| c.is_ascii_whitespace() || matches!(c, ',' | ';' | '(' | '{'))
        .next()
        .unwrap_or("");
    let token = token.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '_');
    if token.is_empty() {
        return false;
    }
    let lower = token.to_ascii_lowercase();
    ["password", "passwd", "secret", "token", "api_key", "apikey"]
        .iter()
        .any(|key| lower.contains(key))
        || lower.contains("authorization")
}

fn connect_host<'a>(check: &'a DueCheck, host: Option<&'a str>) -> Result<&'a str, Outcome> {
    if let Some(host) = host.map(str::trim).filter(|h| !h.is_empty()) {
        if host_ok(host) {
            return Ok(host);
        }
        return Err(Outcome::fail("probe host is invalid"));
    }
    match check
        .address
        .as_deref()
        .map(str::trim)
        .filter(|a| !a.is_empty())
    {
        Some(addr) if host_ok(addr) => Ok(addr),
        Some(_) => Err(Outcome::fail("sandbox address is invalid")),
        None => Err(Outcome::fail("pod has no address")),
    }
}

fn host_ok(host: &str) -> bool {
    !host.is_empty()
        && !host
            .chars()
            .any(|c| c.is_ascii_control() || c == ' ' || c == '/')
        && host.len() <= 253
}

async fn tcp_probe(check: &DueCheck, port: u16) -> Outcome {
    if port == 0 {
        return Outcome::fail("probe port is unset");
    }
    let host = match connect_host(check, None) {
        Ok(host) => host.to_owned(),
        Err(outcome) => return outcome,
    };
    match tokio::time::timeout(check.timeout, TcpStream::connect((host.as_str(), port))).await {
        Ok(Ok(_)) => Outcome::ok(),
        Ok(Err(e)) => Outcome::fail(format!("connect: {e}")),
        Err(_) => Outcome::fail("timed out"),
    }
}

async fn http_probe(
    check: &DueCheck,
    path: &str,
    port: u16,
    host: Option<&str>,
    https: bool,
) -> Outcome {
    if port == 0 {
        return Outcome::fail("probe port is unset");
    }
    let Ok(path) = http_path(path) else {
        return Outcome::fail("probe path is invalid");
    };
    let connect = match connect_host(check, host) {
        Ok(host) => host.to_owned(),
        Err(outcome) => return outcome,
    };
    let header_host = host
        .map(str::trim)
        .filter(|h| !h.is_empty())
        .unwrap_or(connect.as_str())
        .to_owned();
    let result = tokio::time::timeout(check.timeout, async {
        let stream = TcpStream::connect((connect.as_str(), port))
            .await
            .map_err(|e| format!("connect: {e}"))?;
        if https {
            let tls = tls_config().map_err(|e| format!("tls: {e}"))?;
            let name = rustls::pki_types::ServerName::try_from(connect.clone())
                .map_err(|_| "probe host is invalid".to_owned())?;
            let stream = tokio_rustls::TlsConnector::from(tls)
                .connect(name, stream)
                .await
                .map_err(|e| format!("tls: {e}"))?;
            http_status(stream, &path, &header_host).await
        } else {
            http_status(stream, &path, &header_host).await
        }
    })
    .await;
    match result {
        Ok(Ok(code)) if (200..400).contains(&code) => Outcome::ok(),
        Ok(Ok(code)) => Outcome::fail(format!("http status {code}")),
        Ok(Err(message)) => Outcome::fail(message),
        Err(_) => Outcome::fail("timed out"),
    }
}

fn http_path(path: &str) -> Result<String, ()> {
    let path = if path.is_empty() { "/" } else { path };
    if path.chars().any(|c| c.is_ascii_control() || c == ' ') {
        return Err(());
    }
    if path.starts_with('/') {
        Ok(path.to_owned())
    } else {
        Ok(format!("/{path}"))
    }
}

async fn http_status<S>(mut stream: S, path: &str, host: &str) -> Result<u16, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    let request = format!(
        "GET {path} HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: pistack-probe\r\n\r\n"
    );
    stream
        .write_all(request.as_bytes())
        .await
        .map_err(|e| format!("write: {e}"))?;
    let mut buf = [0u8; 128];
    let mut n = 0;
    while n < buf.len() && !buf[..n].windows(2).any(|w| w == b"\r\n") {
        let read = stream
            .read(&mut buf[n..])
            .await
            .map_err(|e| format!("read: {e}"))?;
        if read == 0 {
            break;
        }
        n += read;
    }
    parse_status(&buf[..n]).ok_or_else(|| "http response has no status".to_owned())
}

fn parse_status(buf: &[u8]) -> Option<u16> {
    let text = std::str::from_utf8(buf).ok()?;
    let line = text.lines().next()?;
    let mut parts = line.split_whitespace();
    let version = parts.next()?;
    if !version.starts_with("HTTP/") {
        return None;
    }
    parts.next()?.parse().ok()
}

fn tls_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    static CELL: OnceLock<Result<std::sync::Arc<rustls::ClientConfig>, String>> = OnceLock::new();
    CELL.get_or_init(build_tls_config).clone()
}

fn build_tls_config() -> Result<std::sync::Arc<rustls::ClientConfig>, String> {
    let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let verifier = std::sync::Arc::new(AcceptAnyCert {
        provider: provider.clone(),
    });
    let config = rustls::ClientConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|e| e.to_string())?
        .dangerous()
        .with_custom_certificate_verifier(verifier)
        .with_no_client_auth();
    Ok(std::sync::Arc::new(config))
}

/// Pod probes talk to the sandbox itself. Certificate names on that
/// address are not a trust decision, so the handshake checks the signature
/// and accepts any certificate.
#[derive(Debug)]
struct AcceptAnyCert {
    provider: std::sync::Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for AcceptAnyCert {
    fn verify_server_cert(
        &self,
        _end_entity: &rustls::pki_types::CertificateDer<'_>,
        _intermediates: &[rustls::pki_types::CertificateDer<'_>],
        _server_name: &rustls::pki_types::ServerName<'_>,
        _ocsp_response: &[u8],
        _now: rustls::pki_types::UnixTime,
    ) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        Ok(rustls::client::danger::ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls12_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn verify_tls13_signature(
        &self,
        message: &[u8],
        cert: &rustls::pki_types::CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &self.provider.signature_verification_algorithms,
        )
    }

    fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
        self.provider
            .signature_verification_algorithms
            .supported_schemes()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::kind;

    fn t0() -> DateTime<Utc> {
        DateTime::from_timestamp(1_700_000_000, 0).unwrap()
    }

    fn later(base: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
        base + ChronoDuration::seconds(seconds)
    }

    fn spec(period: u32, delay: u32) -> ProbeSpec {
        ProbeSpec {
            http_get: Some(HttpGetSpec {
                path: "/health".into(),
                port: 8080,
                host: None,
                scheme: None,
            }),
            tcp_socket: None,
            exec: None,
            initial_delay_seconds: delay,
            period_seconds: period,
            timeout_seconds: 1,
            success_threshold: 1,
            failure_threshold: 3,
        }
    }

    fn target(id: &Id) -> ProbeTarget {
        ProbeTarget {
            id: id.clone(),
            power_on: true,
            address: Some("127.0.0.1".into()),
        }
    }

    #[test]
    fn scheduling_waits_for_delay_then_honors_period() {
        let book = ProbeBook::new();
        let id = Id::new(kind::MACHINE);
        let start = t0();
        book.install(
            &id,
            vec![ContainerProbes {
                container: "web".into(),
                liveness: Some(spec(10, 5)),
                readiness: None,
            }],
            start,
        );
        assert!(book.due(&[target(&id)], start).is_empty());
        assert!(book.due(&[target(&id)], later(start, 4)).is_empty());
        let first = book.due(&[target(&id)], later(start, 5));
        assert_eq!(first.len(), 1);
        assert!(book.due(&[target(&id)], later(start, 5)).is_empty());
        assert!(book.due(&[target(&id)], later(start, 14)).is_empty());
        let second = book.due(&[target(&id)], later(start, 15));
        assert_eq!(second.len(), 1);
        assert_eq!(second[0].kind, PROBE_LIVENESS);
    }

    #[test]
    fn streaks_reset_and_exec_does_not_count_as_failure() {
        let book = ProbeBook::new();
        let id = Id::new(kind::MACHINE);
        let start = t0();
        book.install(
            &id,
            vec![ContainerProbes {
                container: "web".into(),
                liveness: Some(ProbeSpec {
                    http_get: None,
                    tcp_socket: None,
                    exec: Some(ExecSpec {
                        command: vec!["/bin/true".into()],
                    }),
                    initial_delay_seconds: 0,
                    period_seconds: 1,
                    timeout_seconds: 1,
                    success_threshold: 1,
                    failure_threshold: 3,
                }),
                readiness: Some(spec(1, 0)),
            }],
            start,
        );
        let due = book.due(&[target(&id)], start);
        assert_eq!(due.len(), 2);
        for check in &due {
            if check.kind == PROBE_LIVENESS {
                book.record(
                    check,
                    &Outcome::unsupported("exec probes are not supported"),
                );
            } else {
                book.record(check, &Outcome::fail("http status 500"));
            }
        }
        let reports = book.reports(&id);
        let live = reports.iter().find(|r| r.kind == PROBE_LIVENESS).unwrap();
        assert_eq!(live.failures, 0);
        assert_eq!(live.successes, 0);
        assert!(live.unsupported);
        assert_eq!(live.message, "exec probes are not supported");
        let ready = reports.iter().find(|r| r.kind == PROBE_READINESS).unwrap();
        assert_eq!(ready.failures, 1);
        assert_eq!(ready.successes, 0);

        let again = book.due(&[target(&id)], later(start, 1));
        let ready_check = again.iter().find(|c| c.kind == PROBE_READINESS).unwrap();
        book.record(ready_check, &Outcome::ok());
        let ready = book
            .reports(&id)
            .into_iter()
            .find(|r| r.kind == PROBE_READINESS)
            .unwrap();
        assert_eq!(ready.successes, 1);
        assert_eq!(ready.failures, 0);
        assert!(ready.success);
    }

    #[test]
    fn install_ignores_a_result_from_the_previous_generation() {
        let book = ProbeBook::new();
        let id = Id::new(kind::MACHINE);
        let start = t0();
        book.install(
            &id,
            vec![ContainerProbes {
                container: "web".into(),
                liveness: Some(spec(1, 0)),
                readiness: None,
            }],
            start,
        );
        let stale = book.due(&[target(&id)], start);
        book.install(
            &id,
            vec![ContainerProbes {
                container: "web".into(),
                liveness: Some(spec(1, 0)),
                readiness: None,
            }],
            later(start, 1),
        );
        book.record(&stale[0], &Outcome::fail("late"));
        let report = &book.reports(&id)[0];
        assert_eq!(report.failures, 0);
        assert!(report.message.is_empty());
    }

    #[test]
    fn sandbox_document_yields_only_containers_with_probes() {
        let raw = r#"{
            "kind": "PodSandbox",
            "containers": [
                {"name": "web", "image": "img", "readiness_probe": {"tcp_socket": {"port": 80}}},
                {"name": "side", "image": "img"},
                {"name": "job", "liveness_probe": {"exec": {"command": ["/bin/true"]}}}
            ]
        }"#;
        let found = probes_in_user_data(Some(raw));
        assert_eq!(found.len(), 2);
        assert_eq!(found[0].container, "web");
        assert!(found[0].readiness.is_some());
        assert_eq!(found[1].container, "job");
        assert!(probes_in_user_data(Some("not json")).is_empty());
        assert!(probes_in_user_data(Some(r#"{"kind":"Other"}"#)).is_empty());
    }

    #[tokio::test]
    async fn http_and_tcp_probes_hit_local_listeners() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = [0u8; 512];
                    let _ = sock.read(&mut buf).await;
                    let text = std::str::from_utf8(&buf).unwrap_or("");
                    let status = if text.contains("GET /ready") {
                        200
                    } else {
                        500
                    };
                    let body = format!(
                        "HTTP/1.1 {status} X\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    );
                    let _ = sock.write_all(body.as_bytes()).await;
                });
            }
        });
        let tcp = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let tcp_addr = tcp.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                if tcp.accept().await.is_err() {
                    break;
                }
            }
        });

        let base = DueCheck {
            machine: Id::new(kind::MACHINE),
            generation: 1,
            container: "web".into(),
            kind: PROBE_READINESS.into(),
            action: ProbeAction::Tcp {
                port: tcp_addr.port(),
            },
            timeout: Duration::from_secs(2),
            address: Some(addr.ip().to_string()),
        };
        assert!(execute(&base).await.success);

        let mut closed = base.clone();
        closed.action = ProbeAction::Tcp { port: 1 };
        let refused = execute(&closed).await;
        assert!(!refused.success);
        assert!(!refused.unsupported);

        let mut ready = base.clone();
        ready.action = ProbeAction::Http {
            path: "/ready".into(),
            port: addr.port(),
            host: None,
            https: false,
        };
        assert!(execute(&ready).await.success);

        let mut bad = ready.clone();
        bad.action = ProbeAction::Http {
            path: "/live".into(),
            port: addr.port(),
            host: None,
            https: false,
        };
        let failed = execute(&bad).await;
        assert!(!failed.success);
        assert_eq!(failed.message, "http status 500");

        let mut exec = base.clone();
        exec.action = ProbeAction::Exec {
            command: vec!["/bin/true".into()],
        };
        let unsupported = execute(&exec).await;
        assert!(unsupported.unsupported);
        assert!(!unsupported.success);
        assert_eq!(unsupported.message, "exec probes are not supported");
    }

    fn exec_check(command: Vec<String>, timeout: Duration) -> DueCheck {
        DueCheck {
            machine: Id::new(kind::MACHINE),
            generation: 1,
            container: "web".into(),
            kind: PROBE_LIVENESS.into(),
            action: ProbeAction::Exec { command },
            timeout,
            address: Some("127.0.0.1".into()),
        }
    }

    #[test]
    fn exec_argv_rejects_an_empty_command_and_a_nul_byte() {
        assert_eq!(
            validate_exec_argv(&[]).unwrap_err(),
            "exec command is empty"
        );
        assert_eq!(
            validate_exec_argv(&[String::new()]).unwrap_err(),
            "exec command is empty"
        );
        assert_eq!(
            validate_exec_argv(&["/bin/echo".into(), String::new()]).unwrap_err(),
            "exec command is empty"
        );
        let nul = format!("a{}b", '\0');
        assert_eq!(
            validate_exec_argv(&["/bin/echo".into(), nul]).unwrap_err(),
            "exec command contains a NUL byte"
        );
        assert!(validate_exec_argv(&["/bin/true".into()]).is_ok());
    }

    #[tokio::test]
    async fn exec_probe_maps_exit_timeout_and_drops_secrets() {
        let check = exec_check(vec!["/bin/check".into()], Duration::from_secs(2));
        let ok = run(&check, |argv, timeout| {
            assert_eq!(argv, &["/bin/check".to_string()]);
            assert_eq!(timeout, Duration::from_secs(2));
            async {
                CommandProbeReply::Finished {
                    exit_code: 0,
                    output: "password=hunter2".into(),
                }
            }
        })
        .await;
        assert!(ok.success);
        assert!(!ok.unsupported);
        assert!(ok.message.is_empty());

        let failed = run(&check, |_, _| async {
            CommandProbeReply::Finished {
                exit_code: 7,
                output: "password=hunter2 status=down".into(),
            }
        })
        .await;
        assert!(!failed.success);
        assert!(!failed.unsupported);
        assert!(failed.message.starts_with("exit 7"), "{}", failed.message);
        assert!(failed.message.contains("status=down"), "{}", failed.message);
        assert!(!failed.message.contains("hunter2"), "{}", failed.message);
        assert!(failed.message.contains("***"), "{}", failed.message);

        let timed_out = run(&check, |_, _| async { CommandProbeReply::TimedOut }).await;
        assert!(!timed_out.success);
        assert!(!timed_out.unsupported);
        assert_eq!(timed_out.message, "timed out");

        let from_youki = outcome_of_reply(reply_from_exec(Err(ApiError::unavailable(
            "youki runtime: exec timed out after 1s",
        ))));
        assert_eq!(from_youki.message, "timed out");
        assert!(!from_youki.success);

        let missing = outcome_of_reply(reply_from_exec(Err(ApiError::not_found(
            "youki runtime: machine mach-1 is not on this host",
        ))));
        assert!(missing.unsupported);
        assert!(!missing.success);

        let key = outcome_of_reply(CommandProbeReply::Finished {
            exit_code: 1,
            output: "-----BEGIN OPENSSH PRIVATE KEY-----\nabc\n".into(),
        });
        assert_eq!(key.message, "exit 1");
        assert!(!key.message.contains("abc"));
        assert!(!key.message.contains("PRIVATE"));

        let leaked = outcome_of_reply(reply_from_exec(Err(ApiError::unavailable(
            "youki runtime: exec failed: token=s3cret-value",
        ))));
        assert!(!leaked.unsupported);
        assert!(
            !leaked.message.contains("s3cret-value"),
            "{}",
            leaked.message
        );

        let long = "status=down ".repeat(40);
        let truncated = run(&check, |_, _| {
            let output = long.clone();
            async move {
                CommandProbeReply::Finished {
                    exit_code: 2,
                    output,
                }
            }
        })
        .await;
        let excerpt = truncated
            .message
            .strip_prefix("exit 2: ")
            .expect("failure keeps the exit code");
        assert_eq!(excerpt.chars().count(), PROBE_MESSAGE_CAP);
        assert!(excerpt.contains("status=down"));
        assert!(excerpt.chars().count() < PROBE_EXEC_OUTPUT_CAP);
    }

    #[tokio::test]
    async fn exec_validation_does_not_call_the_runtime() {
        let called = std::sync::atomic::AtomicBool::new(false);
        let empty = exec_check(vec![], Duration::from_secs(1));
        let outcome = run(&empty, |_, _| {
            called.store(true, std::sync::atomic::Ordering::Relaxed);
            async {
                CommandProbeReply::Finished {
                    exit_code: 0,
                    output: String::new(),
                }
            }
        })
        .await;
        assert!(!called.load(std::sync::atomic::Ordering::Relaxed));
        assert!(!outcome.success);
        assert!(!outcome.unsupported);
        assert_eq!(outcome.message, "exec command is empty");

        let nul = exec_check(vec![format!("a{}b", '\0')], Duration::from_secs(1));
        let outcome = run(&nul, |_, _| async {
            CommandProbeReply::Finished {
                exit_code: 0,
                output: String::new(),
            }
        })
        .await;
        assert_eq!(outcome.message, "exec command contains a NUL byte");
        assert!(!outcome.success);
    }

    #[tokio::test]
    async fn exec_streaks_follow_period_and_thresholds() {
        let book = ProbeBook::new();
        let id = Id::new(kind::MACHINE);
        let start = t0();
        let spec = ProbeSpec {
            http_get: None,
            tcp_socket: None,
            exec: Some(ExecSpec {
                command: vec!["/bin/check".into()],
            }),
            initial_delay_seconds: 5,
            period_seconds: 10,
            timeout_seconds: 2,
            success_threshold: 2,
            failure_threshold: 3,
        };
        book.install(
            &id,
            vec![ContainerProbes {
                container: "web".into(),
                liveness: Some(spec.clone()),
                readiness: Some(spec.clone()),
            }],
            start,
        );
        assert!(book.due(&[target(&id)], start).is_empty());

        async fn settle(book: &ProbeBook, id: &Id, at: DateTime<Utc>, reply: CommandProbeReply) {
            for check in book.due(&[target(id)], at) {
                let outcome = run(&check, |argv, timeout| {
                    assert_eq!(argv, &["/bin/check".to_string()]);
                    assert_eq!(timeout, Duration::from_secs(2));
                    let reply = reply.clone();
                    async move { reply }
                })
                .await;
                book.record(&check, &outcome);
            }
        }

        settle(
            &book,
            &id,
            later(start, 5),
            CommandProbeReply::Finished {
                exit_code: 1,
                output: String::new(),
            },
        )
        .await;
        assert!(book.due(&[target(&id)], later(start, 5)).is_empty());
        assert!(book.due(&[target(&id)], later(start, 14)).is_empty());

        settle(&book, &id, later(start, 15), CommandProbeReply::TimedOut).await;
        let live = book
            .reports(&id)
            .into_iter()
            .find(|r| r.kind == PROBE_LIVENESS)
            .unwrap();
        assert_eq!(live.failures, 2);
        assert!(live.failures < spec.failure_threshold);
        assert!(!live.success);
        assert!(!live.unsupported);
        assert_eq!(live.message, "timed out");

        settle(
            &book,
            &id,
            later(start, 25),
            CommandProbeReply::Finished {
                exit_code: 9,
                output: String::new(),
            },
        )
        .await;
        let live = book
            .reports(&id)
            .into_iter()
            .find(|r| r.kind == PROBE_LIVENESS)
            .unwrap();
        assert_eq!(live.failures, spec.failure_threshold);
        assert_eq!(live.message, "exit 9");

        settle(
            &book,
            &id,
            later(start, 35),
            CommandProbeReply::Finished {
                exit_code: 0,
                output: String::new(),
            },
        )
        .await;
        let ready = book
            .reports(&id)
            .into_iter()
            .find(|r| r.kind == PROBE_READINESS)
            .unwrap();
        assert_eq!(ready.successes, 1);
        assert_eq!(ready.failures, 0);
        assert!(ready.successes < spec.success_threshold);
        assert!(ready.success);

        settle(
            &book,
            &id,
            later(start, 45),
            CommandProbeReply::Finished {
                exit_code: 0,
                output: String::new(),
            },
        )
        .await;
        let ready = book
            .reports(&id)
            .into_iter()
            .find(|r| r.kind == PROBE_READINESS)
            .unwrap();
        assert_eq!(ready.successes, spec.success_threshold);
        assert_eq!(ready.failures, 0);

        settle(&book, &id, later(start, 55), CommandProbeReply::Unsupported).await;
        let live = book
            .reports(&id)
            .into_iter()
            .find(|r| r.kind == PROBE_LIVENESS)
            .unwrap();
        assert!(live.unsupported);
        assert_eq!(live.failures, 0);
        assert_eq!(live.successes, 0);
        assert!(!live.success);
    }
}
