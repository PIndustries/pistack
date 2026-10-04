// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! rustls configuration from `keep`-issued material, so the binary can put
//! TLS (and mTLS) on the gateway and on agent connections without knowing
//! anything about PEM.
//!
//! The crypto provider is pinned to aws-lc-rs explicitly. Other crates in
//! the workspace enable rustls's `ring` feature, and with two providers
//! compiled in rustls refuses to pick a default.

use rustls::pki_types::{pem::PemObject, CertificateDer, PrivateKeyDer};
use rustls::server::WebPkiClientVerifier;
use rustls::{ClientConfig, RootCertStore, ServerConfig};
use std::sync::Arc;

use crate::error::{KeepError, Result};

/// A certificate, its private key and the CA bundle to trust peers with.
#[derive(Clone, Debug)]
pub struct Pki {
    pub cert_pem: String,
    pub key_pem: String,
    /// Issuing CA then root, PEM concatenated.
    pub ca_pem: String,
}

fn provider() -> Arc<rustls::crypto::CryptoProvider> {
    Arc::new(rustls::crypto::aws_lc_rs::default_provider())
}

/// All certificates in a PEM string.
pub fn certs_from_pem(pem_str: &str) -> Result<Vec<CertificateDer<'static>>> {
    let certs = CertificateDer::pem_slice_iter(pem_str.as_bytes())
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(|e| KeepError::Tls(format!("bad certificate PEM: {e}")))?;
    if certs.is_empty() {
        return Err(KeepError::Tls("no certificates in PEM".into()));
    }
    Ok(certs)
}

fn key_from_pem(pem_str: &str) -> Result<PrivateKeyDer<'static>> {
    PrivateKeyDer::from_pem_slice(pem_str.as_bytes())
        .map_err(|e| KeepError::Tls(format!("bad private key PEM: {e}")))
}

fn roots_from_pem(ca_pem: &str) -> Result<Arc<RootCertStore>> {
    let mut roots = RootCertStore::empty();
    for c in certs_from_pem(ca_pem)? {
        roots
            .add(c)
            .map_err(|e| KeepError::Tls(format!("bad CA certificate: {e}")))?;
    }
    Ok(Arc::new(roots))
}

/// Server side. Presents `pki.cert_pem` plus the CA chain. Client
/// certificates are requested and, when offered, must chain to `pki.ca_pem`;
/// a client with no certificate is still admitted so bearer-token clients
/// keep working. Use [`peer_common_name`] after the handshake to find out
/// which it was.
pub fn server_config(pki: &Pki) -> Result<ServerConfig> {
    let mut chain = certs_from_pem(&pki.cert_pem)?;
    chain.extend(certs_from_pem(&pki.ca_pem)?);
    let key = key_from_pem(&pki.key_pem)?;
    let verifier =
        WebPkiClientVerifier::builder_with_provider(roots_from_pem(&pki.ca_pem)?, provider())
            .allow_unauthenticated()
            .build()
            .map_err(|e| KeepError::Tls(format!("client verifier: {e}")))?;
    let mut cfg = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_client_cert_verifier(verifier)
        .with_single_cert(chain, key)?;
    cfg.alpn_protocols = vec![b"h2".to_vec(), b"http/1.1".to_vec()];
    Ok(cfg)
}

/// Gateway HTTPS for browsers and bearer tokens.
///
/// Unlike [`server_config`], this does not ask for a client certificate, so
/// a browser does not open a certificate picker. ALPN is HTTP/1.1 only: the
/// axum listener is built without the `http2` feature, and advertising `h2`
/// makes browsers speak a protocol the process cannot read.
pub fn server_config_http(pki: &Pki) -> Result<ServerConfig> {
    let mut chain = certs_from_pem(&pki.cert_pem)?;
    chain.extend(certs_from_pem(&pki.ca_pem)?);
    let key = key_from_pem(&pki.key_pem)?;
    let mut cfg = ServerConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(chain, key)?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(cfg)
}

/// Client side. Trusts `ca_pem`; with `client` set, presents that
/// certificate for mTLS.
pub fn client_config(ca_pem: &str, client: Option<&Pki>) -> Result<ClientConfig> {
    let builder = ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .with_root_certificates(roots_from_pem(ca_pem)?);
    let cfg = match client {
        Some(p) => {
            let mut chain = certs_from_pem(&p.cert_pem)?;
            chain.extend(certs_from_pem(&p.ca_pem)?);
            builder.with_client_auth_cert(chain, key_from_pem(&p.key_pem)?)?
        }
        None => builder.with_no_client_auth(),
    };
    Ok(cfg)
}

/// Common name of the peer's leaf certificate, if it presented one.
pub fn peer_common_name(conn: &rustls::CommonState) -> Option<String> {
    conn.peer_certificates()
        .and_then(|c| c.first())
        .and_then(|leaf| crate::ca::common_name_from_der(leaf).ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ca::Ca;
    use crate::crypto::RootKey;
    use crate::types::{CertUsage, CertificateSpec};
    use pistack_store::Store;
    use rustls::pki_types::ServerName;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio_rustls::{TlsAcceptor, TlsConnector};

    fn pki(ca: &Ca, cn: &str, sans: Vec<String>, usage: CertUsage) -> Pki {
        let issued = ca
            .issue(&CertificateSpec {
                common_name: cn.into(),
                sans,
                usage,
                ttl_days: 7,
                issuer: None,
            })
            .unwrap();
        Pki {
            cert_pem: issued.cert_pem,
            key_pem: issued.key_pem,
            ca_pem: ca.bundle_pem(),
        }
    }

    #[tokio::test]
    async fn mtls_handshake_recovers_client_cn() {
        let store = Store::in_memory().unwrap();
        let key = RootKey::load_or_create(&store).unwrap();
        let ca = Ca::load_or_init(&store, &key).unwrap();

        let server = pki(&ca, "gateway", vec!["127.0.0.1".into()], CertUsage::Server);
        let client = pki(&ca, "agent-7", vec![], CertUsage::Client);

        let acceptor = TlsAcceptor::from(Arc::new(server_config(&server).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let srv = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut tls = acceptor.accept(tcp).await.unwrap();
            let cn = peer_common_name(tls.get_ref().1);
            let mut buf = [0u8; 4];
            tls.read_exact(&mut buf).await.unwrap();
            tls.write_all(b"pong").await.unwrap();
            tls.shutdown().await.ok();
            (cn, buf)
        });

        let connector = TlsConnector::from(Arc::new(
            client_config(&ca.bundle_pem(), Some(&client)).unwrap(),
        ));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
            .await
            .unwrap();
        assert_eq!(
            peer_common_name(tls.get_ref().1).as_deref(),
            Some("gateway")
        );
        tls.write_all(b"ping").await.unwrap();
        let mut buf = [0u8; 4];
        tls.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"pong");
        let (cn, got) = srv.await.unwrap();
        assert_eq!(cn.as_deref(), Some("agent-7"));
        assert_eq!(&got, b"ping");
    }

    #[tokio::test]
    async fn no_client_cert_is_admitted_and_foreign_ca_is_not() {
        let store = Store::in_memory().unwrap();
        let key = RootKey::load_or_create(&store).unwrap();
        let ca = Ca::load_or_init(&store, &key).unwrap();
        let server = pki(&ca, "gateway", vec!["localhost".into()], CertUsage::Server);
        let acceptor = TlsAcceptor::from(Arc::new(server_config(&server).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let mut cns = Vec::new();
            for _ in 0..2 {
                let (tcp, _) = listener.accept().await.unwrap();
                match acceptor.accept(tcp).await {
                    Ok(mut tls) => {
                        cns.push(peer_common_name(tls.get_ref().1));
                        tls.shutdown().await.ok();
                    }
                    Err(_) => cns.push(Some("handshake-failed".into())),
                }
            }
            cns
        });

        // Bearer-token style client: trusted CA, no cert.
        let connector =
            TlsConnector::from(Arc::new(client_config(&ca.bundle_pem(), None).unwrap()));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .unwrap();
        tls.shutdown().await.ok();

        // A client that does not trust our CA fails to connect.
        let other_store = Store::in_memory().unwrap();
        let other_key = RootKey::load_or_create(&other_store).unwrap();
        let other = Ca::load_or_init(&other_store, &other_key).unwrap();
        let connector =
            TlsConnector::from(Arc::new(client_config(&other.bundle_pem(), None).unwrap()));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        assert!(connector
            .connect(ServerName::try_from("localhost").unwrap(), tcp)
            .await
            .is_err());

        let cns = srv.await.unwrap();
        assert_eq!(cns[0], None);
        assert_eq!(cns[1].as_deref(), Some("handshake-failed"));
    }
}
