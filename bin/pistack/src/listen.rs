// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Plain HTTP plus an optional rustls listener on a specific address.
//!
//! Certificate files live in `data_dir/tls/`. When `cert.pem`, `key.pem`,
//! and `ca.pem` are all present and non-empty they are reused. Otherwise
//! `keep` issues a server certificate for CN `pistack`.
//!
//! The TLS listener requests client certificates and verifies any that are
//! offered against the cloud CA. Verified peer identity is published as
//! [`keep::PeerIdentity`] via axum `ConnectInfo` so `/rpc` can require mTLS.

use anyhow::Context;
use axum::extract::connect_info::Connected;
use axum::extract::ConnectInfo;
use axum::extract::Request;
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::serve::{IncomingStream, Listener};
use keep::PeerIdentity;
use pistack_store::Store;
use std::net::{IpAddr, SocketAddr};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::net::{TcpListener, TcpStream};
use tokio_rustls::TlsAcceptor;
use tokio_util::sync::CancellationToken;

/// Local wrapper so we can implement axum `Connected` (orphan rules).
#[derive(Clone, Debug)]
struct GatewayPeer(PeerIdentity);

/// HTTPS bind plus the store whose CA signs a missing certificate.
pub struct TlsListen {
    pub addr: String,
    pub data_dir: PathBuf,
    pub store: Store,
}

pub struct BoundListeners {
    pub http: SocketAddr,
    pub tls: Option<SocketAddr>,
    app: axum::Router,
    http_listener: TcpListener,
    tls_listener: Option<TlsListener>,
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
                    let peer = PeerIdentity::from_tls(addr, tls.get_ref().1);
                    return (tls, GatewayPeer(peer));
                }
                Err(e) => {
                    tracing::debug!(%addr, error = %e, "tls handshake failed");
                }
            }
        }
    }

    fn local_addr(&self) -> std::io::Result<GatewayPeer> {
        Ok(GatewayPeer(PeerIdentity {
            addr: Some(self.tcp.local_addr()?),
            common_name: None,
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
        GatewayPeer(PeerIdentity::plain(*stream.remote_addr()))
    }
}

/// Copy [`PeerIdentity`] into request extensions for `/rpc` middleware.
async fn publish_peer(
    ConnectInfo(peer): ConnectInfo<GatewayPeer>,
    mut req: Request,
    next: Next,
) -> Response {
    req.extensions_mut().insert(peer.0);
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

    let (tls_listener, tls_bound) = if let (Some(spec), Some(addr)) = (tls, tls_addr) {
        let pki = load_or_issue(&spec.data_dir, &spec.store, addr.ip())?;
        let cfg = keep::tls::server_config_http(&pki).context("building rustls server config")?;
        let acceptor = TlsAcceptor::from(Arc::new(cfg));
        let tcp = TcpListener::bind(addr)
            .await
            .with_context(|| format!("binding tls listener {addr}"))?;
        let bound = tcp.local_addr()?;
        tracing::info!(tls_listen = %bound, "tls listener");
        (Some(TlsListener { tcp, acceptor }), Some(bound))
    } else {
        (None, None)
    };

    Ok(BoundListeners {
        http,
        tls: tls_bound,
        app,
        http_listener,
        tls_listener,
    })
}

impl BoundListeners {
    /// Serve until `shutdown` is cancelled. Both listeners watch that token.
    pub async fn serve(self, shutdown: CancellationToken) -> anyhow::Result<()> {
        let app = self.app.layer(middleware::from_fn(publish_peer));
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

fn load_or_issue(data_dir: &Path, store: &Store, ip: IpAddr) -> anyhow::Result<keep::Pki> {
    let dir = data_dir.join("tls");
    let cert_path = dir.join("cert.pem");
    let key_path = dir.join("key.pem");
    let ca_path = dir.join("ca.pem");
    if let Some(pki) = read_existing(&cert_path, &key_path, &ca_path)? {
        restrict_key(&key_path)?;
        tracing::info!("reusing tls certificate files");
        return Ok(pki);
    }
    let client = keep::Client::open(store).context("opening keep for the server certificate")?;
    let sans = vec![ip.to_string(), "pistack".to_string()];
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

        shutdown.cancel();
        task.await.expect("serve task").expect("serve");
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
}
