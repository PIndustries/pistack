// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Liveness and readiness checks run on the host that holds the pod.
//!
//! The agent reads probe definitions from the sandbox document (or from
//! the runtime, after a restart) and connects to the pod's sandbox
//! address. Results are consecutive streaks reported on the ordinary
//! machine heartbeat. An `exec` handler is stored and reported
//! unsupported; it does not count as a failed check.

use chrono::{DateTime, Duration as ChronoDuration, Utc};
use pistack_rpc::proto::{ProbeReport, PROBE_LIVENESS, PROBE_READINESS};
use pistack_types::Id;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;

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
    Exec,
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
        if self.exec.is_some() {
            return ProbeAction::Exec;
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
    message: String,
}

impl Track {
    fn note(&mut self, outcome: &Outcome) {
        if outcome.unsupported {
            self.successes = 0;
            self.failures = 0;
            self.success = false;
            self.message = outcome.message.clone();
            return;
        }
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

/// Run one check. `exec` and a probe with no handler are unsupported and
/// do not count as a failed streak.
pub async fn execute(check: &DueCheck) -> Outcome {
    match &check.action {
        ProbeAction::Exec => Outcome::unsupported("exec probes are not supported"),
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
        exec.action = ProbeAction::Exec;
        let unsupported = execute(&exec).await;
        assert!(unsupported.unsupported);
        assert_eq!(unsupported.message, "exec probes are not supported");
    }
}
