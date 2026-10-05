// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! TLS material for a terminated listener.
//!
//! The certificate and its private key are read from `keep`. This module
//! does not issue, renew, or generate a certificate. A missing, revoked, or
//! unreadable certificate is unavailable.

use base64::Engine;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};
use rustls::pki_types::pem::PemObject;
use rustls::pki_types::{CertificateDer, PrivateKeyDer};
use std::sync::Arc;

/// Leaf certificate and private key already stored by `keep`.
pub struct TlsMaterial {
    pub cert_pem: String,
    pub key_pem: String,
}

impl std::fmt::Debug for TlsMaterial {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TlsMaterial")
            .field("cert_pem", &self.cert_pem)
            .field("key_pem", &"<redacted>")
            .finish()
    }
}

impl Clone for TlsMaterial {
    fn clone(&self) -> Self {
        Self {
            cert_pem: self.cert_pem.clone(),
            key_pem: self.key_pem.clone(),
        }
    }
}

const NO_CERT: &str = "no certificate for this listener";

/// Read `id` from `keep`. Does not create a root key, a CA, or a certificate.
pub fn load_kept(store: &Store, id: &Id) -> ApiResult<TlsMaterial> {
    let cert = store
        .collection::<keep::Certificate>()
        .get(id)
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    if cert.status.revoked || cert.status.cert_pem.trim().is_empty() {
        return Err(ApiError::unavailable(NO_CERT));
    }
    let key_id = cert
        .status
        .key_secret
        .as_ref()
        .ok_or_else(|| ApiError::unavailable(NO_CERT))?;
    let secret = store
        .collection::<keep::StoredSecret>()
        .get(key_id)
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let sealed = secret
        .status
        .sealed
        .as_ref()
        .ok_or_else(|| ApiError::unavailable(NO_CERT))?;
    let root = existing_root(store)?;
    let key = root
        .open(sealed, key_id.to_string().as_bytes())
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let key_pem = String::from_utf8(key).map_err(|_| ApiError::unavailable(NO_CERT))?;
    if key_pem.trim().is_empty() {
        return Err(ApiError::unavailable(NO_CERT));
    }
    Ok(TlsMaterial {
        cert_pem: cert.status.cert_pem,
        key_pem,
    })
}

fn existing_root(store: &Store) -> ApiResult<keep::RootKey> {
    let Some(b64) = store
        .get_meta::<String>(keep::crypto::META_ROOT_KEY)
        .map_err(|_| ApiError::unavailable(NO_CERT))?
    else {
        return Err(ApiError::unavailable(NO_CERT));
    };
    let raw = base64::engine::general_purpose::STANDARD
        .decode(b64.trim())
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let bytes: [u8; 32] = raw.try_into().map_err(|_| ApiError::unavailable(NO_CERT))?;
    Ok(keep::RootKey::from_bytes(bytes))
}

/// rustls config that presents `material`. Parsing a bad PEM is unavailable.
/// Nothing here calls the CA.
pub fn server_config(material: &TlsMaterial) -> ApiResult<Arc<rustls::ServerConfig>> {
    let certs = CertificateDer::pem_slice_iter(material.cert_pem.as_bytes())
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    if certs.is_empty() {
        return Err(ApiError::unavailable(NO_CERT));
    }
    let key = PrivateKeyDer::from_pem_slice(material.key_pem.as_bytes())
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    let provider = Arc::new(rustls::crypto::aws_lc_rs::default_provider());
    let mut cfg = rustls::ServerConfig::builder_with_provider(provider)
        .with_safe_default_protocol_versions()
        .map_err(|_| ApiError::unavailable(NO_CERT))?
        .with_no_client_auth()
        .with_single_cert(certs, key)
        .map_err(|_| ApiError::unavailable(NO_CERT))?;
    cfg.alpn_protocols = vec![b"http/1.1".to_vec()];
    Ok(Arc::new(cfg))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::provider::{Providers, RelayL7Provider};
    use crate::types::*;
    use crate::Client;
    use keep::CertificateSpec;
    use pistack_store::Store;
    use pistack_types::error::ErrorCode;
    use pistack_types::{kind, Meta, Phase};
    use rustls::pki_types::ServerName;
    use std::net::{IpAddr, Ipv4Addr, SocketAddr};
    use std::sync::Arc;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio_rustls::TlsConnector;

    #[test]
    fn missing_certificate_does_not_generate_one() {
        let store = Store::in_memory().unwrap();
        let err = load_kept(&store, &Id::new(kind::CERT)).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("no certificate"));
        assert!(store
            .get_meta::<String>(keep::crypto::META_ROOT_KEY)
            .unwrap()
            .is_none());
    }

    #[test]
    fn reads_a_stored_certificate_and_does_not_issue_another() {
        let store = Store::in_memory().unwrap();
        let keep = keep::Client::open(&store).unwrap();
        let cert = keep
            .issue_certificate(
                None,
                "lb",
                CertificateSpec {
                    common_name: "lb.pistack.local".into(),
                    sans: vec!["127.0.0.1".into()],
                    ..CertificateSpec::default()
                },
            )
            .unwrap();
        let before = keep.list_certificates(None).unwrap().len();
        let material = load_kept(&store, &cert.meta.id).unwrap();
        assert!(material.cert_pem.contains("BEGIN CERTIFICATE"));
        assert!(material.key_pem.contains("PRIVATE KEY"));
        assert_eq!(keep.list_certificates(None).unwrap().len(), before);
        server_config(&material).unwrap();

        let mut revoked = cert.clone();
        keep.revoke_certificate(&revoked.meta.id).unwrap();
        let err = load_kept(&store, &revoked.meta.id).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        let _ = &mut revoked;
    }

    #[tokio::test]
    async fn terminated_listener_serves_the_stored_certificate() {
        let member = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let member_addr = member.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = member.accept().await.unwrap();
            let mut buf = vec![0u8; 2048];
            let n = sock.read(&mut buf).await.unwrap();
            let req = String::from_utf8_lossy(&buf[..n]);
            assert!(req.starts_with("GET / HTTP/1.1"), "{req}");
            let body = b"pong";
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            sock.write_all(resp.as_bytes()).await.unwrap();
            sock.write_all(body).await.unwrap();
        });

        let store = Store::in_memory().unwrap();
        let keep = keep::Client::open(&store).unwrap();
        let issued = keep
            .issue_certificate(
                None,
                "lb",
                CertificateSpec {
                    common_name: "lb.pistack.local".into(),
                    sans: vec!["127.0.0.1".into()],
                    ..CertificateSpec::default()
                },
            )
            .unwrap();
        let before = keep.list_certificates(None).unwrap().len();
        let provider = Arc::new(RelayL7Provider::new());
        let client = Client::new(store.clone(), Providers::new(vec![provider]));
        let project = Id::new(kind::PROJECT);
        let lb = client
            .create_balancer(
                Meta::new(kind::BALANCER, "lb", Some(project.clone())),
                BalancerSpec {
                    vip_network: "net".into(),
                    vip_address: Some(IpAddr::V4(Ipv4Addr::LOCALHOST)),
                    vip_port: None,
                    floating_ip: None,
                    provider: Provider::Relay,
                    admin_up: true,
                },
            )
            .unwrap();
        let pool = client
            .create_pool(
                Meta::new(kinds::POOL, "pool", Some(project.clone())),
                PoolSpec {
                    balancer: lb.meta.id.clone(),
                    protocol: PoolProtocol::Http,
                    algorithm: Algorithm::default(),
                    session_persistence: None,
                    health_monitor: None,
                    admin_up: true,
                },
            )
            .unwrap();
        client
            .create_member(
                Meta::new(kinds::MEMBER, "m", Some(project.clone())),
                MemberSpec {
                    pool: pool.meta.id.clone(),
                    address: member_addr.ip(),
                    port: member_addr.port(),
                    weight: 1,
                    backup: false,
                    admin_up: true,
                },
            )
            .unwrap();

        let missing = client
            .create_listener(
                Meta::new(kinds::LISTENER, "missing", Some(project.clone())),
                ListenerSpec {
                    balancer: lb.meta.id.clone(),
                    protocol: ListenerProtocol::Terminated {
                        cert: Id::new(kind::CERT),
                    },
                    port: 443,
                    default_pool: Some(pool.meta.id.clone()),
                    connection_limit: 0,
                    admin_up: true,
                    door: None,
                    routes: Vec::new(),
                },
            )
            .unwrap();
        client.reconcile_balancer(&lb.meta.id).await;
        let missing = client.listeners().get(&missing.meta.id).unwrap();
        assert_ne!(missing.status.phase, Phase::Ready);
        assert!(missing.status.message.contains("no certificate"));
        assert_eq!(keep.list_certificates(None).unwrap().len(), before);

        let listener = client
            .create_listener(
                Meta::new(kinds::LISTENER, "https", Some(project)),
                ListenerSpec {
                    balancer: lb.meta.id.clone(),
                    protocol: ListenerProtocol::Terminated {
                        cert: issued.meta.id.clone(),
                    },
                    port: 8443,
                    default_pool: Some(pool.meta.id.clone()),
                    connection_limit: 0,
                    admin_up: true,
                    door: None,
                    routes: Vec::new(),
                },
            )
            .unwrap();
        client.reconcile_listener(&listener.meta.id).await;
        let listener = client.listeners().get(&listener.meta.id).unwrap();
        assert_eq!(
            listener.status.phase,
            Phase::Ready,
            "{}",
            listener.status.message
        );
        assert_eq!(keep.list_certificates(None).unwrap().len(), before);

        let bound: SocketAddr = listener.status.provider_ref.unwrap().parse().unwrap();
        let ca = keep.ca().bundle_pem();
        let cfg = keep::tls::client_config(&ca, None).unwrap();
        let connector = TlsConnector::from(Arc::new(cfg));
        let tcp = tokio::net::TcpStream::connect(bound).await.unwrap();
        let mut tls = connector
            .connect(
                ServerName::IpAddress(std::net::IpAddr::V4(Ipv4Addr::LOCALHOST).into()),
                tcp,
            )
            .await
            .unwrap();
        tls.write_all(b"GET / HTTP/1.1\r\nHost: lb\r\n\r\n")
            .await
            .unwrap();
        let mut buf = Vec::new();
        tokio::time::timeout(std::time::Duration::from_secs(5), tls.read_to_end(&mut buf))
            .await
            .expect("timed out")
            .unwrap();
        assert!(buf.windows(4).any(|w| w == b"pong"), "{buf:?}");
    }
}
