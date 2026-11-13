// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Plain HTTP plus an optional rustls listener on a specific address.
//!
//! Certificate files live in `data_dir/tls/`. When `cert.pem`, `key.pem`,
//! and `ca.pem` are all present and non-empty they are reused. Otherwise
//! `keep` issues a server certificate for CN `pistack`, valid for the
//! listen address and for the host agents and other servers are told to
//! call. A certificate the cloud CA issued that lacks one of those names
//! is issued again; one from another CA is the operator's and is left.
//!
//! The certificate is renewed while the server runs ([`CertRenewal`]):
//! once two thirds of its lifetime have passed the server makes a new key,
//! has the cloud CA certify it, replaces the files and presents the new
//! certificate from the next handshake on. No restart, and connections
//! already open are not cut. The leader issues it itself; a follower asks
//! the leader, because only the leader writes the store. A certificate in
//! `tls/` that this cloud did not issue is never replaced: renewal fails
//! with a warning and the operator replaces the files and restarts.
//!
//! The TLS listener requests client certificates and verifies any that are
//! offered against the cloud CA. Verified peer identity is published as
//! [`keep::PeerIdentity`] via axum `ConnectInfo` so `/rpc` can require mTLS.

use anyhow::Context;
use axum::extract::connect_info::Connected;
use axum::extract::Request;
use axum::extract::{ConnectInfo, State};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::serve::{IncomingStream, Listener};
use chrono::{DateTime, Utc};
use keep::PeerIdentity;
use pistack_store::Store;
use pistack_types::Leadership;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

/// Local wrapper so we can implement axum `Connected` (orphan rules).
#[derive(Clone, Debug)]
struct GatewayPeer {
    peer: PeerIdentity,
    /// The client's leaf certificate, when it offered one that chained to
    /// the cloud CA. Kept so each request can check it is not revoked.
    leaf: Option<Arc<Vec<u8>>>,
}

impl GatewayPeer {
    fn plain(peer: PeerIdentity) -> Self {
        GatewayPeer { peer, leaf: None }
    }
}

/// HTTPS bind plus the store whose CA signs a missing certificate.
pub struct TlsListen {
    pub addr: String,
    pub data_dir: PathBuf,
    pub store: Store,
    /// Hosts the certificate must be valid for besides the listen address:
    /// the one in `--agent-url`, which is where other servers call too.
    pub names: Vec<String>,
    /// Whether this server may issue its own renewal or must ask.
    pub leadership: Leadership,
    /// This server's id in the member list, to find the leader with.
    pub node_id: String,
}

pub struct BoundListeners {
    pub http: SocketAddr,
    pub tls: Option<SocketAddr>,
    app: axum::Router,
    /// The store the revocation list is read from; set with TLS.
    store: Option<Store>,
    http_listener: TcpListener,
    tls_listener: Option<TlsListener>,
    renewal: Option<CertRenewal>,
}

struct TlsListener {
    tcp: TcpListener,
    acceptor: TlsAcceptor,
}

impl Listener for TlsListener {
    type Io = tokio_rustls::server::TlsStream<TcpStream>;
    type Addr = GatewayPeer;

    async fn accept(&mut self) -> (Self::Io, Self::Addr) {
        loop {
            let (sock, addr) = match self.tcp.accept().await {
                Ok(v) => v,
                Err(e) => {
                    tracing::error!(error = %e, "tls accept failed");
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                    continue;
                }
            };
            match self.acceptor.accept(sock).await {
                Ok(tls) => {
                    let conn = tls.get_ref().1;
                    let peer = PeerIdentity::from_tls(addr, conn);
                    let leaf = conn
                        .peer_certificates()
                        .and_then(|c| c.first())
                        .map(|c| Arc::new(c.as_ref().to_vec()));
                    return (tls, GatewayPeer { peer, leaf });
                }
                Err(e) => {
                    tracing::debug!(%addr, error = %e, "tls handshake failed");
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<GatewayPeer> {
        Ok(GatewayPeer::plain(PeerIdentity {
            addr: Some(self.tcp.local_addr()?),
            common_name: None,
            tls: true,
        }))
    }
}

impl Connected<IncomingStream<'_, TlsListener>> for GatewayPeer {
    fn connect_info(stream: IncomingStream<'_, TlsListener>) -> Self {
        stream.remote_addr().clone()
    }
}

impl Connected<IncomingStream<'_, TcpListener>> for GatewayPeer {
    fn connect_info(stream: IncomingStream<'_, TcpListener>) -> Self {
        GatewayPeer::plain(PeerIdentity::plain(*stream.remote_addr()))
    }
}

/// Copy [`PeerIdentity`] into request extensions for `/rpc` middleware.
///
/// A client certificate is checked against the revocation list on every
/// request, not once per connection, so revoking one cuts off an agent that
/// is already connected. A revoked certificate, or a list that cannot be
/// read, leaves the request with no peer name, which `/rpc` refuses.
async fn publish_peer(
    State(store): State<Option<Store>>,
    ConnectInfo(conn): ConnectInfo<GatewayPeer>,
    mut req: Request,
    next: Next,
) -> Response {
    let mut peer = conn.peer;
    if let (Some(store), Some(leaf)) = (&store, &conn.leaf) {
        match keep::ca::is_revoked(store, leaf) {
            Ok(false) => {}
            Ok(true) => {
                tracing::warn!(peer = ?peer.common_name, "revoked client certificate refused");
                peer.common_name = None;
            }
            Err(e) => {
                tracing::error!(error = %e, "cannot read the revocation list");
                peer.common_name = None;
            }
        }
    }
    req.extensions_mut().insert(peer);
    next.run(req).await
}

/// Bind plain HTTP, and TLS when `tls` is set. A wildcard TLS address or a
/// failed bind returns an error and does not serve.
pub async fn bind_listeners(
    app: axum::Router,
    http_listen: &str,
    tls: Option<TlsListen>,
) -> anyhow::Result<BoundListeners> {
    let tls_addr = match &tls {
        Some(t) => Some(parse_tls_addr(&t.addr)?),
        None => None,
    };
    let http_listener = TcpListener::bind(http_listen)
        .await
        .with_context(|| format!("binding {http_listen}"))?;
    let http = http_listener.local_addr()?;

    let mut renewal = None;
    let (tls_listener, tls_bound, store) = if let (Some(spec), Some(addr)) = (tls, tls_addr) {
        let pki = load_or_issue(&spec.data_dir, &spec.store, addr.ip(), &spec.names)?;
        let cert = keep::tls::ServerCert::new(&pki).context("loading the tls certificate")?;
        let cfg = keep::tls::server_config_http_reloadable(&pki.ca_pem, cert.clone())
            .context("building rustls server config")?;
        renewal = Some(CertRenewal {
            dir: spec.data_dir.join("tls"),
            store: spec.store.clone(),
            leadership: spec.leadership,
            node_id: spec.node_id,
            cert,
            http: reqwest::Client::builder()
                .connect_timeout(std::time::Duration::from_secs(5))
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .unwrap_or_else(|_| reqwest::Client::new()),
        });
        let acceptor = TlsAcceptor::from(Arc::new(cfg));
        let tcp = TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding tls listener {addr}"))?;
        let bound = tcp.local_addr()?;
        tracing::info!(tls_listen = %bound, "tls listener");
        (
            Some(TlsListener { tcp, acceptor }),
            Some(bound),
            Some(spec.store),
        )
    } else {
        (None, None, None)
    };

    Ok(BoundListeners {
        http,
        tls: tls_bound,
        app,
        store,
        http_listener,
        tls_listener,
        renewal,
    })
}

/// Pause before the first look at the listener certificate, between
/// looks, and after a renewal that did not go through.
const CERT_FIRST_CHECK: std::time::Duration = std::time::Duration::from_secs(60);
const CERT_CHECK_EVERY: std::time::Duration = std::time::Duration::from_secs(60 * 60);
const CERT_RETRY_AFTER: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// Renews the listener certificate in `tls/` and hands the new one to the
/// running listener.
#[derive(Clone)]
struct CertRenewal {
    dir: PathBuf,
    store: Store,
    leadership: Leadership,
    node_id: String,
    cert: Arc<keep::tls::ServerCert>,
    http: reqwest::Client,
}

#[derive(Debug, PartialEq, Eq)]
enum CertStep {
    NotDue {
        renew_at: DateTime<Utc>,
    },
    Renewed {
        serial: u64,
        not_after: DateTime<Utc>,
    },
}

impl CertRenewal {
    async fn run(self, shutdown: CancellationToken) {
        let mut wait = CERT_FIRST_CHECK;
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => return,
                _ = tokio::time::sleep(wait) => {}
            }
            wait = match self.check(Utc::now()).await {
                Ok(CertStep::NotDue { .. }) => CERT_CHECK_EVERY,
                Ok(CertStep::Renewed { serial, not_after }) => {
                    tracing::info!(serial, expires = %not_after, "tls listener certificate renewed and in use");
                    CERT_CHECK_EVERY
                }
                Err(e) => {
                    tracing::warn!(
                        error = %format!("{e:#}"),
                        retry_in = ?CERT_RETRY_AFTER,
                        "tls listener certificate was not renewed and the current one stays in use; \
                         if this cloud did not issue it, replace the files in tls/ and restart, \
                         or remove tls/ and restart to have one issued"
                    );
                    CERT_RETRY_AFTER
                }
            };
        }
    }

    /// Renew the certificate if it is due at `now`.
    async fn check(&self, now: DateTime<Utc>) -> anyhow::Result<CertStep> {
        let pki = keep::files::load(&self.dir)?.context("the tls certificate files are gone")?;
        let der = keep::ca::pem_to_der(&pki.cert_pem)?;
        let (not_before, not_after) = keep::ca::validity(&der)?;
        let renew_at = keep::ca::renew_at(not_before, not_after);
        if now < renew_at {
            return Ok(CertStep::NotDue { renew_at });
        }
        let serial = keep::ca::serial_from_der(&der)?;
        let cn = keep::ca::common_name_from_der(&der)?;
        // The key is made here and stays here.
        let request = keep::ca::new_key_request(&cn)?;
        let issued = if self.leadership.is_leader() {
            let (store, csr) = (self.store.clone(), request.csr_pem.clone());
            tokio::task::spawn_blocking(move || {
                keep::Client::open(&store)?.renew_certificate(serial, &cn, &csr)
            })
            .await
            .context("renewal task")??
        } else {
            self.ask_leader(serial, &cn, &request.csr_pem).await?
        };
        let new = keep::Pki {
            cert_pem: issued.cert_pem,
            key_pem: request.key_pem,
            ca_pem: issued.ca_pem,
        };
        // Loaded once before anything is written: a certificate that does
        // not fit the key never replaces a working one.
        keep::tls::ServerCert::new(&new).context("the renewed certificate does not load")?;
        keep::files::stage(&self.dir, &new)?;
        keep::files::commit(&self.dir)?;
        self.cert.replace(&new)?;
        let der = keep::ca::pem_to_der(&new.cert_pem)?;
        Ok(CertStep::Renewed {
            serial: keep::ca::serial_from_der(&der)?,
            not_after: keep::ca::validity(&der)?.1,
        })
    }

    /// Have the leader renew it. This server proves it is a member with
    /// the peer key, as on every other call between servers.
    async fn ask_leader(&self, serial: u64, cn: &str, csr_pem: &str) -> anyhow::Result<keep::Pki> {
        let leader = crate::cluster::leader_hint(&self.store, &self.node_id)?
            .context("no leader is known to ask for a certificate")?;
        let key =
            crate::cluster::peer_key(&self.store).context("this server has no peer key yet")?;
        let url = format!(
            "{}/secrets/peer/renew-certificate",
            leader.trim_end_matches('/')
        );
        let resp = self
            .http
            .post(&url)
            .header(crate::cluster::PEER_HEADER, key)
            .json(&keep::api::PeerRenew {
                serial,
                common_name: cn.to_owned(),
                csr_pem: csr_pem.to_owned(),
            })
            .send()
            .await
            .with_context(|| format!("asking the leader at {leader}"))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            anyhow::bail!("the leader at {leader} answered {status}: {body}");
        }
        let answer: keep::api::PeerRenewed = resp.json().await?;
        Ok(keep::Pki {
            cert_pem: answer.cert_pem,
            key_pem: String::new(),
            ca_pem: answer.ca_pem,
        })
    }
}

impl BoundListeners {
    /// Serve until `shutdown` is cancelled. Both listeners watch that token.
    pub async fn serve(self, shutdown: CancellationToken) -> anyhow::Result<()> {
        if let Some(renewal) = self.renewal {
            tokio::spawn(renewal.run(shutdown.clone()));
        }
        let app = self
            .app
            .layer(middleware::from_fn_with_state(self.store, publish_peer));
        let http_sd = shutdown.clone();
        let http_svc = app
            .clone()
            .into_make_service_with_connect_info::<GatewayPeer>();
        let http = axum::serve(self.http_listener, http_svc)
            .with_graceful_shutdown(async move { http_sd.cancelled().await });
        match self.tls_listener {
            Some(tls_listener) => {
                let tls_sd = shutdown;
                let tls_svc = app.into_make_service_with_connect_info::<GatewayPeer>();
                let tls = axum::serve(tls_listener, tls_svc)
                    .with_graceful_shutdown(async move { tls_sd.cancelled().await });
                let (http_res, tls_res) = tokio::join!(http, tls);
                http_res.context("http listener")?;
                tls_res.context("tls listener")?;
                Ok(())
            }
            None => {
                http.await.context("http listener")?;
                Ok(())
            }
        }
    }
}

fn parse_tls_addr(addr: &str) -> anyhow::Result<SocketAddr> {
    let sock: SocketAddr = addr
        .parse()
        .with_context(|| format!("tls listen address '{addr}' must be a specific ip:port"))?;
    if sock.ip().is_unspecified() {
        anyhow::bail!("refusing wildcard tls listen {sock}; bind a specific address");
    }
    Ok(sock)
}

/// What a server certificate must be valid for: the address it listens on,
/// the name `pistack`, and any host other machines are told to call.
fn wanted_names(ip: IpAddr, extra: &[String]) -> Vec<String> {
    let mut names = vec![ip.to_string(), "pistack".to_string()];
    for name in extra {
        let name = name.trim().trim_matches(|c| c == '[' || c == ']');
        if !name.is_empty() && !names.iter().any(|have| have == name) {
            names.push(name.to_owned());
        }
    }
    names
}

/// Whether the certificate on disk can stay.
#[derive(Debug, PartialEq, Eq)]
enum Existing {
    Keep,
    /// Not issued by this cloud's CA. It is the operator's; other servers
    /// and agents cannot check it.
    Foreign,
    /// Issued by this cloud's CA but not valid for these names.
    Lacks(Vec<String>),
}

fn judge_existing(pki: &keep::Pki, store: &Store, want: &[String]) -> anyhow::Result<Existing> {
    let Some(cloud_root) = keep::ca::root_fingerprint(store)? else {
        return Ok(Existing::Keep);
    };
    let chain = keep::tls::certs_from_pem(&pki.ca_pem)?;
    let ours = chain
        .iter()
        .any(|cert| keep::crypto::sha256_hex(cert.as_ref()) == cloud_root);
    if !ours {
        return Ok(Existing::Foreign);
    }
    let leaf = keep::ca::pem_to_der(&pki.cert_pem)?;
    let have = keep::ca::names_from_der(&leaf)?;
    let missing: Vec<String> = want
        .iter()
        .filter(|name| !have.contains(name))
        .cloned()
        .collect();
    if missing.is_empty() {
        Ok(Existing::Keep)
    } else {
        Ok(Existing::Lacks(missing))
    }
}

fn load_or_issue(
    data_dir: &Path,
    store: &Store,
    ip: IpAddr,
    extra: &[String],
) -> anyhow::Result<keep::Pki> {
    let dir = data_dir.join("tls");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    let sans = wanted_names(ip, extra);
    if keep::files::recover(&dir).context("finishing an interrupted certificate renewal")? {
        tracing::info!("finished a tls certificate renewal that was interrupted");
    }
    if let Some(pki) = read_existing(&cert_path, &key_path, &ca_path)? {
        match judge_existing(&pki, store, &sans).context("reading the tls certificate files")? {
            Existing::Keep => {
                restrict_key(&key_path)?;
                tracing::info!("reusing tls certificate files");
                return Ok(pki);
            }
            Existing::Foreign => {
                restrict_key(&key_path)?;
                tracing::warn!(
                    "reusing tls certificate files that this cloud's CA did not issue: other servers and agents check this server against the cloud CA and will refuse it. Remove the tls directory to have one issued"
                );
                return Ok(pki);
            }
            Existing::Lacks(missing) => tracing::info!(
                missing = %missing.join(", "),
                "the tls certificate is not valid for every address this server is called at; issuing a new one"
            ),
        }
    }
    let client = keep::Client::open(store).context("opening keep for the server certificate")?;
    let pki = client
        .issue_server_cert("pistack", &sans)
        .context("issuing server certificate")?;
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    write_pem(&cert_path, &pki.cert_pem)?;
    write_secret(&key_path, &pki.key_pem)?;
    write_pem(&ca_path, &pki.ca_pem)?;
    tracing::info!("issued tls certificate");
    Ok(pki)
}

fn read_existing(cert: &Path, key: &Path, ca: &Path) -> anyhow::Result<Option<keep::Pki>> {
    if !(non_empty(cert) && non_empty(key) && non_empty(ca)) {
        return Ok(None);
    }
    Ok(Some(keep::Pki {
        cert_pem: std::fs::read_to_string(cert).context("reading tls certificate")?,
        key_pem: std::fs::read_to_string(key).context("reading tls key")?,
        ca_pem: std::fs::read_to_string(ca).context("reading tls ca")?,
    }))
}

fn non_empty(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.len() > 0)
        .unwrap_or(false)
}

fn write_pem(path: &Path, pem: &str) -> anyhow::Result<()> {
    std::fs::write(path, pem).with_context(|| format!("writing {}", path.display()))
}

fn write_secret(path: &Path, pem: &str) -> anyhow::Result<()> {
    {
        use std::io::Write;
        use std::os::unix::fs::OpenOptionsExt;
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(path)
            .with_context(|| format!("writing {}", path.display()))?;
        file.write_all(pem.as_bytes())?;
    }
    restrict_key(path)
}

fn restrict_key(path: &Path) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .with_context(|| format!("setting mode 0600 on {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::routing::get;
    use pistack_types::{Module, ModuleContext};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn tls_listen_refuses_unspecified_addresses() {
        for addr in ["0.0.0.0:8443", "[::]:8443", "0.0.0.0:0", "[::]:0"] {
            let err = parse_tls_addr(addr).expect_err(addr);
            assert!(
                err.to_string().contains("refusing wildcard"),
                "{addr}: {err}"
            );
        }
        assert!(parse_tls_addr("127.0.0.1:8443").is_ok());
        assert!(parse_tls_addr("[::1]:8443").is_ok());
    }

    #[tokio::test]
    async fn http_and_tls_both_answer() {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let (events, _) = tokio::sync::broadcast::channel(16);
        let store = Store::open(data.join("state.redb"), events.clone()).expect("store");
        keep::Client::open(&store).expect("ca");

        let ctx = ModuleContext {
            store: Arc::new(store.clone()),
            events,
            shutdown: CancellationToken::new(),
            public_url: "http://127.0.0.1".into(),
            node_name: "test".into(),
            leadership: pistack_types::Leadership::always(),
        };
        let app = axum::Router::new()
            .route("/healthz", get(|| async { "ok" }))
            .nest("/network", fabric::Fabric::default().router(ctx));

        let shutdown = CancellationToken::new();
        let bound = bind_listeners(
            app,
            "127.0.0.1:0",
            Some(TlsListen {
                addr: "127.0.0.1:0".into(),
                data_dir: data.clone(),
                store,
                names: Vec::new(),
                leadership: pistack_types::Leadership::always(),
                node_id: "n-test".into(),
            }),
        )
        .await
        .expect("bind");
        let http = bound.http;
        let tls = bound.tls.expect("tls addr");
        let sd = shutdown.clone();
        let task = tokio::spawn(async move { bound.serve(sd).await });

        let (status, body) = http_get(http, "/healthz").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("ok"), "{body}");

        let ca = std::fs::read_to_string(data.join("tls/ca.pem")).expect("ca.pem");
        assert!(!ca.is_empty());
        let key_mode = {
            use std::os::unix::fs::PermissionsExt;
            std::fs::metadata(data.join("tls/key.pem"))
                .expect("key.pem")
                .permissions()
                .mode()
                & 0o777
        };
        assert_eq!(key_mode, 0o600);
        assert!(std::fs::metadata(data.join("tls/cert.pem")).unwrap().len() > 0);

        let (status, body) = https_get(tls, &ca, "/healthz").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("ok"), "{body}");

        let (status, body) = https_get(tls, &ca, "/network/networks").await;
        assert_eq!(status, 401, "{body}");
        assert!(body.contains("\"unauthorized\""), "{body}");
        assert!(body.contains("no credentials"), "{body}");

        let (status, body) = http_get(http, "/healthz").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("ok"), "{body}");

        shutdown.cancel();
        task.await.expect("serve task").expect("serve");
    }

    #[tokio::test]
    async fn tls_peer_identity_exposes_client_cn() {
        use axum::routing::get;

        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let (events, _) = tokio::sync::broadcast::channel(16);
        let store = Store::open(data.join("state.redb"), events).expect("store");
        keep::Client::open(&store).expect("ca");

        let app = axum::Router::new().route(
            "/whoami",
            get(
                |axum::Extension(peer): axum::Extension<PeerIdentity>| async move {
                    peer.common_name.unwrap_or_else(|| "none".into())
                },
            ),
        );

        let shutdown = CancellationToken::new();
        let bound = bind_listeners(
            app,
            "127.0.0.1:0",
            Some(TlsListen {
                addr: "127.0.0.1:0".into(),
                data_dir: data.clone(),
                store: store.clone(),
                names: Vec::new(),
                leadership: pistack_types::Leadership::always(),
                node_id: "n-test".into(),
            }),
        )
        .await
        .expect("bind");
        let tls = bound.tls.expect("tls addr");
        let sd = shutdown.clone();
        let task = tokio::spawn(async move { bound.serve(sd).await });

        let ca = std::fs::read_to_string(data.join("tls/ca.pem")).expect("ca.pem");
        let keep = keep::Client::open(&store).expect("keep");
        let client_pki = keep.issue_client_cert("agent-who").expect("client cert");

        let (status, body) = https_get_with_client(tls, &ca, Some(&client_pki), "/whoami").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("agent-who"), "{body}");

        let (status, body) = https_get_with_client(tls, &ca, None, "/whoami").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("none"), "{body}");

        // Revoked: the handshake still completes, the name is gone.
        let certs = store
            .collection::<keep::Certificate>()
            .list(None)
            .expect("certificates");
        let cert = certs
            .iter()
            .find(|c| c.spec.common_name == "agent-who")
            .expect("the agent's certificate record");
        keep.revoke_certificate(&cert.meta.id).expect("revoke");
        let (status, body) = https_get_with_client(tls, &ca, Some(&client_pki), "/whoami").await;
        assert_eq!(status, 200, "{body}");
        assert!(body.contains("none"), "{body}");

        shutdown.cancel();
        task.await.expect("serve task").expect("serve");
    }

    /// Serial of the certificate the listener presents right now.
    async fn served_serial(addr: SocketAddr, ca_pem: &str) -> u64 {
        let cfg = keep::tls::client_config(ca_pem, None).expect("client config");
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tcp = TcpStream::connect(addr).await.expect("tls connect");
        let name =
            tokio_rustls::rustls::pki_types::ServerName::try_from("pistack").expect("server name");
        let tls = connector.connect(name, tcp).await.expect("tls handshake");
        let leaf = tls.get_ref().1.peer_certificates().expect("server chain")[0].clone();
        keep::ca::serial_from_der(&leaf).expect("serial")
    }

    struct Tls {
        data: PathBuf,
        store: Store,
        addr: SocketAddr,
        renewal: CertRenewal,
        shutdown: CancellationToken,
        task: tokio::task::JoinHandle<anyhow::Result<()>>,
        _dir: tempfile::TempDir,
    }

    /// A server with a TLS listener in front of the routes `app` builds
    /// over its store.
    async fn tls_server(app: impl FnOnce(&Store) -> axum::Router, leadership: Leadership) -> Tls {
        let dir = tempfile::tempdir().expect("tempdir");
        let data = dir.path().to_path_buf();
        let (events, _) = tokio::sync::broadcast::channel(16);
        let store = Store::open(data.join("state.redb"), events).expect("store");
        keep::Client::open(&store).expect("ca");
        let shutdown = CancellationToken::new();
        let bound = bind_listeners(
            app(&store),
            "127.0.0.1:0",
            Some(TlsListen {
                addr: "127.0.0.1:0".into(),
                data_dir: data.clone(),
                store: store.clone(),
                names: Vec::new(),
                leadership,
                node_id: "srv-b".into(),
            }),
        )
        .await
        .expect("bind");
        let addr = bound.tls.expect("tls addr");
        let renewal = bound.renewal.clone().expect("renewal");
        let sd = shutdown.clone();
        let task = tokio::spawn(async move { bound.serve(sd).await });
        Tls {
            data,
            store,
            addr,
            renewal,
            shutdown,
            task,
            _dir: dir,
        }
    }

    fn renew_point(cert_pem: &str) -> DateTime<Utc> {
        let der = keep::ca::pem_to_der(cert_pem).expect("der");
        let (nb, na) = keep::ca::validity(&der).expect("validity");
        keep::ca::renew_at(nb, na)
    }

    #[tokio::test]
    async fn the_listener_certificate_is_renewed_and_served_without_a_restart() {
        let app = |_: &Store| axum::Router::new().route("/healthz", get(|| async { "ok" }));
        let t = tls_server(app, Leadership::always()).await;
        let read = |f: &str| std::fs::read_to_string(t.data.join("tls").join(f)).expect(f);
        let ca = read("ca.pem");
        let first_cert = read("cert.pem");
        let first_key = read("key.pem");
        let first = served_serial(t.addr, &ca).await;
        let due = renew_point(&first_cert);

        // Before two thirds of its lifetime nothing happens.
        let early = due - chrono::Duration::seconds(1);
        assert_eq!(
            t.renewal.check(early).await.expect("check"),
            CertStep::NotDue { renew_at: due }
        );
        assert_eq!(read("cert.pem"), first_cert);
        assert_eq!(served_serial(t.addr, &ca).await, first);

        // At the threshold: a new certificate and a new key, on disk and
        // on the listener, which was never restarted.
        let step = t.renewal.check(due).await.expect("renew");
        let CertStep::Renewed { serial, .. } = step else {
            panic!("not renewed: {step:?}");
        };
        assert_ne!(serial, first);
        assert_ne!(read("cert.pem"), first_cert);
        assert_ne!(read("key.pem"), first_key);
        assert!(!t.data.join("tls/next").exists());
        assert_eq!(served_serial(t.addr, &ca).await, serial);
        let (status, body) = https_get(t.addr, &ca, "/healthz").await;
        assert_eq!(status, 200, "{body}");
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(t.data.join("tls/key.pem"))
                .expect("key.pem")
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // The control plane knows the new one replaced the old, holds no
        // key for it, and kept the names.
        let keep = keep::Client::open(&t.store).expect("keep");
        let old = keep.managed_certificate(first).expect("old").expect("old");
        assert_eq!(old.status.renewed_by, Some(serial));
        let new = keep.managed_certificate(serial).expect("new").expect("new");
        assert!(new.status.key_secret.is_none());
        assert_eq!(new.spec.sans, old.spec.sans);

        // A restart reads the renewed files.
        let again = load_or_issue(&t.data, &t.store, t.addr.ip(), &[]).expect("reload");
        assert_eq!(again.cert_pem, read("cert.pem"));

        t.shutdown.cancel();
        t.task.await.expect("serve task").expect("serve");
    }

    #[tokio::test]
    async fn a_follower_has_the_leader_renew_its_listener_certificate() {
        // The follower: it must not issue anything itself.
        let app = |_: &Store| axum::Router::new().route("/healthz", get(|| async { "ok" }));
        let t = tls_server(app, Leadership::new(false)).await;
        let read = |f: &str| std::fs::read_to_string(t.data.join("tls").join(f)).expect(f);
        let ca = read("ca.pem");
        let first = served_serial(t.addr, &ca).await;
        let due = renew_point(&read("cert.pem"));

        // No leader known: nothing changes, and the error says why.
        let err = t.renewal.check(due).await.expect_err("no leader");
        assert!(format!("{err:#}").contains("no leader"), "{err:#}");
        assert_eq!(served_serial(t.addr, &ca).await, first);

        // The leader: keep's routes over the same state, as a replica has.
        let leader_app = axum::Router::new().nest(
            "/secrets",
            keep::api::router(keep::Client::open(&t.store).expect("keep")),
        );
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind leader");
        let leader = format!("http://{}", listener.local_addr().expect("addr"));
        let leader_task = tokio::spawn(async move { axum::serve(listener, leader_app).await });
        gate::Keys::load_or_create(&t.store).expect("signing key and peer secret");
        t.store
            .put_meta(
                "members",
                &vec![
                    pistack_cluster::Member::voter("srv-a", &leader),
                    pistack_cluster::Member::voter("srv-b", "http://127.0.0.1:1"),
                ],
            )
            .expect("members");
        t.store
            .put_meta("cluster_leader_id", &"srv-a".to_owned())
            .expect("leader id");

        let step = t
            .renewal
            .check(due)
            .await
            .expect("renew through the leader");
        let CertStep::Renewed { serial, .. } = step else {
            panic!("not renewed: {step:?}");
        };
        assert_ne!(serial, first);
        assert_eq!(served_serial(t.addr, &ca).await, serial);

        // Without the peer key the leader issues nothing.
        let http = reqwest::Client::new();
        let body = keep::api::PeerRenew {
            serial,
            common_name: "pistack".into(),
            csr_pem: keep::ca::new_key_request("pistack").expect("csr").csr_pem,
        };
        let url = format!("{leader}/secrets/peer/renew-certificate");
        let resp = http.post(&url).json(&body).send().await.expect("send");
        assert_eq!(resp.status(), 401);
        let resp = http
            .post(&url)
            .header(crate::cluster::PEER_HEADER, "guess")
            .json(&body)
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 401);
        // A member cannot get an agent's certificate renewed this way.
        let keep = keep::Client::open(&t.store).expect("keep");
        let agent = keep.issue_client_cert("n1").expect("agent cert");
        let agent_serial =
            keep::ca::serial_from_der(&keep::ca::pem_to_der(&agent.cert_pem).expect("der"))
                .expect("serial");
        let resp = http
            .post(&url)
            .header(
                crate::cluster::PEER_HEADER,
                crate::cluster::peer_key(&t.store).expect("peer key"),
            )
            .json(&keep::api::PeerRenew {
                serial: agent_serial,
                common_name: "n1".into(),
                csr_pem: keep::ca::new_key_request("n1").expect("csr").csr_pem,
            })
            .send()
            .await
            .expect("send");
        assert_eq!(resp.status(), 403);

        leader_task.abort();
        t.shutdown.cancel();
        t.task.await.expect("serve task").expect("serve");
    }

    async fn http_get(addr: SocketAddr, path: &str) -> (u16, String) {
        let mut tcp = TcpStream::connect(addr).await.expect("http connect");
        let req = format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        tcp.write_all(req.as_bytes()).await.expect("write");
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), tcp.read_to_end(&mut buf))
            .await
            .expect("http timeout")
            .expect("read");
        parse_response(&buf)
    }

    async fn https_get(addr: SocketAddr, ca_pem: &str, path: &str) -> (u16, String) {
        https_get_with_client(addr, ca_pem, None, path).await
    }

    async fn https_get_with_client(
        addr: SocketAddr,
        ca_pem: &str,
        client: Option<&keep::Pki>,
        path: &str,
    ) -> (u16, String) {
        let cfg = keep::tls::client_config(ca_pem, client).expect("client config");
        let connector = tokio_rustls::TlsConnector::from(Arc::new(cfg));
        let tcp = TcpStream::connect(addr).await.expect("tls connect");
        let name =
            tokio_rustls::rustls::pki_types::ServerName::try_from("pistack").expect("server name");
        let mut tls = connector.connect(name, tcp).await.expect("tls handshake");
        let req = format!("GET {path} HTTP/1.1\r\nHost: pistack\r\nConnection: close\r\n\r\n");
        tls.write_all(req.as_bytes()).await.expect("write");
        tls.flush().await.expect("flush");
        let mut buf = Vec::new();
        tokio::time::timeout(Duration::from_secs(5), tls.read_to_end(&mut buf))
            .await
            .expect("https timeout")
            .expect("read");
        parse_response(&buf)
    }

    fn parse_response(buf: &[u8]) -> (u16, String) {
        let text = String::from_utf8_lossy(buf).into_owned();
        let status = text
            .split_whitespace()
            .nth(1)
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        (status, text)
    }

    fn names_on_disk(dir: &Path) -> Vec<String> {
        let pem = std::fs::read_to_string(dir.join("tls/cert.pem")).unwrap();
        keep::ca::names_from_der(&keep::ca::pem_to_der(&pem).unwrap()).unwrap()
    }

    #[test]
    fn the_certificate_is_valid_for_every_address_the_server_is_called_at() {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::in_memory().unwrap();
        keep::Client::open(&store).unwrap();
        let ip: IpAddr = "10.0.0.11".parse().unwrap();

        load_or_issue(dir.path(), &store, ip, &[]).unwrap();
        assert_eq!(names_on_disk(dir.path()), ["10.0.0.11", "pistack"]);
        let first = std::fs::read_to_string(dir.path().join("tls/cert.pem")).unwrap();
        // Nothing changed: the same file is used again.
        load_or_issue(dir.path(), &store, ip, &[]).unwrap();
        let again = std::fs::read_to_string(dir.path().join("tls/cert.pem")).unwrap();
        assert_eq!(first, again);

        // Started with --agent-url: the host in it is added.
        let extra = ["cloud.example".to_string(), "10.0.0.11".to_string()];
        load_or_issue(dir.path(), &store, ip, &extra).unwrap();
        assert_eq!(
            names_on_disk(dir.path()),
            ["10.0.0.11", "pistack", "cloud.example"]
        );
        // Moved to another address: issued for that one.
        let moved: IpAddr = "10.0.0.21".parse().unwrap();
        load_or_issue(dir.path(), &store, moved, &[]).unwrap();
        assert_eq!(names_on_disk(dir.path()), ["10.0.0.21", "pistack"]);
    }

    #[test]
    fn a_certificate_from_another_ca_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let other = Store::in_memory().unwrap();
        let ip: IpAddr = "10.0.0.11".parse().unwrap();
        let theirs = keep::Client::open(&other)
            .unwrap()
            .issue_server_cert("console", &["console.example".to_string()])
            .unwrap();
        std::fs::create_dir_all(dir.path().join("tls")).unwrap();
        std::fs::write(dir.path().join("tls/cert.pem"), &theirs.cert_pem).unwrap();
        std::fs::write(dir.path().join("tls/key.pem"), &theirs.key_pem).unwrap();
        std::fs::write(dir.path().join("tls/ca.pem"), &theirs.ca_pem).unwrap();

        let store = Store::in_memory().unwrap();
        keep::Client::open(&store).unwrap();
        let want = wanted_names(ip, &[]);
        assert_eq!(
            judge_existing(&theirs, &store, &want).unwrap(),
            Existing::Foreign
        );
        let used = load_or_issue(dir.path(), &store, ip, &[]).unwrap();
        assert_eq!(used.cert_pem, theirs.cert_pem);
        assert_eq!(names_on_disk(dir.path()), ["console.example"]);
    }
}
