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

/// Gateway HTTPS for browsers, bearer tokens, and agent mTLS.
///
/// Client certificates are requested and, when offered, must chain to
/// `pki.ca_pem`. A client with no certificate is still admitted so the
/// console and bearer-token callers keep working; `/rpc` enforces a peer
/// certificate in application middleware. ALPN is HTTP/1.1 only: the axum
/// listener is built without the `http2` feature, and advertising `h2`
/// makes browsers speak a protocol the process cannot read.
pub fn server_config_http(pki: &Pki) -> Result<ServerConfig> {
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

/// Read a certificate fingerprint as a new server is given it: 64 hex
/// digits of SHA-256, with or without colons, in either case, and with or
/// without a leading `sha256:`.
pub fn parse_fingerprint(text: &str) -> Result<[u8; 32]> {
    let text = text.trim();
    let hex = text
        .strip_prefix("sha256:")
        .or_else(|| text.strip_prefix("SHA256:"))
        .unwrap_or(text);
    let digits: Vec<u8> = hex.bytes().filter(|b| *b != b':').collect();
    let bad = || KeepError::Invalid("a CA fingerprint is 64 hex digits of SHA-256".into());
    if digits.len() != 64 {
        return Err(bad());
    }
    let mut out = [0u8; 32];
    for (byte, pair) in out.iter_mut().zip(digits.chunks(2)) {
        let pair = std::str::from_utf8(pair).map_err(|_| bad())?;
        *byte = u8::from_str_radix(pair, 16).map_err(|_| bad())?;
    }
    Ok(out)
}

/// Checks a server against a CA known only by the SHA-256 of its
/// certificate. The server has to present that CA certificate in its chain;
/// it then becomes the one trust anchor and the rest is the ordinary check:
/// the leaf chains to it, is in date and is valid for the name dialled.
#[derive(Debug)]
struct PinnedCa {
    pin: [u8; 32],
    provider: Arc<rustls::crypto::CryptoProvider>,
}

impl rustls::client::danger::ServerCertVerifier for PinnedCa {
    fn verify_server_cert(
        &self,
        end_entity: &CertificateDer<'_>,
        intermediates: &[CertificateDer<'_>],
        server_name: &rustls::pki_types::ServerName<'_>,
        ocsp_response: &[u8],
        now: rustls::pki_types::UnixTime,
    ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
        use sha2::{Digest, Sha256};
        use subtle::ConstantTimeEq;
        let anchor = intermediates
            .iter()
            .find(|cert| bool::from(Sha256::digest(cert.as_ref()).as_slice().ct_eq(&self.pin)))
            .ok_or_else(|| {
                rustls::Error::General(
                    "the server did not present a CA certificate with the pinned fingerprint"
                        .into(),
                )
            })?;
        let mut roots = RootCertStore::empty();
        roots.add(anchor.clone().into_owned())?;
        let inner = rustls::client::WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            self.provider.clone(),
        )
        .build()
        .map_err(|e| rustls::Error::General(format!("pinned CA: {e}")))?;
        inner.verify_server_cert(end_entity, intermediates, server_name, ocsp_response, now)
    }

    fn verify_tls12_signature(
        &self,
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
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
        cert: &CertificateDer<'_>,
        dss: &rustls::DigitallySignedStruct,
    ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
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

/// Client side for a server that has nothing of the cloud yet but the
/// fingerprint of its root CA. See [`parse_fingerprint`].
pub fn client_config_pinned(pin: [u8; 32]) -> Result<ClientConfig> {
    let verifier = PinnedCa {
        pin,
        provider: provider(),
    };
    Ok(ClientConfig::builder_with_provider(provider())
        .with_safe_default_protocol_versions()?
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(verifier))
        .with_no_client_auth())
}

/// Peer identity recovered after a TLS handshake on the gateway listener.
///
/// `common_name` is set when the client presented a certificate that chained
/// to the cloud CA. Plain HTTP and clients that offer no certificate leave it
/// empty; `/rpc` treats a missing name as unauthenticated when strict.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PeerIdentity {
    pub addr: Option<std::net::SocketAddr>,
    pub common_name: Option<String>,
    /// The request came in on the TLS listener, with or without a
    /// certificate.
    pub tls: bool,
}

impl PeerIdentity {
    pub fn from_tls(addr: std::net::SocketAddr, conn: &rustls::CommonState) -> Self {
        Self {
            addr: Some(addr),
            common_name: peer_common_name(conn),
            tls: true,
        }
    }

    pub fn plain(addr: std::net::SocketAddr) -> Self {
        Self {
            addr: Some(addr),
            common_name: None,
            tls: false,
        }
    }
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

    #[test]
    fn a_fingerprint_is_read_in_the_forms_people_paste() {
        let hex = "00112233445566778899aabbccddeeff00112233445566778899AABBCCDDEEFF";
        let want = parse_fingerprint(hex).unwrap();
        assert_eq!(want[1], 0x11);
        assert_eq!(want[31], 0xff);
        assert_eq!(parse_fingerprint(&format!("sha256:{hex}")).unwrap(), want);
        let colons: Vec<String> = hex
            .as_bytes()
            .chunks(2)
            .map(|pair| String::from_utf8_lossy(pair).into_owned())
            .collect();
        assert_eq!(parse_fingerprint(&colons.join(":")).unwrap(), want);
        assert_eq!(parse_fingerprint(&format!("  {hex}\n")).unwrap(), want);
        for bad in [
            "",
            "abcd",
            &hex[..62],
            &format!("{hex}00"),
            &hex.replace('0', "g"),
        ] {
            assert!(parse_fingerprint(bad).is_err(), "{bad}");
        }
    }

    /// One TLS connection to a server holding `server`, dialled as `name`
    /// by a client that knows only `pin`.
    async fn dial_pinned(server: &Pki, name: &str, pin: [u8; 32]) -> bool {
        let acceptor = TlsAcceptor::from(Arc::new(server_config_http(server).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            if let Ok(mut tls) = acceptor.accept(tcp).await {
                tls.shutdown().await.ok();
            }
        });
        let connector = TlsConnector::from(Arc::new(client_config_pinned(pin).unwrap()));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let connected = connector
            .connect(ServerName::try_from(name.to_owned()).unwrap(), tcp)
            .await;
        let ok = connected.is_ok();
        if let Ok(mut tls) = connected {
            tls.shutdown().await.ok();
        }
        srv.await.unwrap();
        ok
    }

    #[tokio::test]
    async fn a_pinned_client_accepts_only_the_cloud_with_that_root() {
        let store = Store::in_memory().unwrap();
        let key = RootKey::load_or_create(&store).unwrap();
        let ca = Ca::load_or_init(&store, &key).unwrap();
        let pin =
            parse_fingerprint(&crate::ca::root_fingerprint(&store).unwrap().unwrap()).unwrap();
        let server = pki(&ca, "pistack", vec!["127.0.0.1".into()], CertUsage::Server);
        assert!(dial_pinned(&server, "127.0.0.1", pin).await);
        // The right cloud, but not a certificate for the address dialled.
        assert!(!dial_pinned(&server, "10.9.9.9", pin).await);
        assert!(!dial_pinned(&server, "other.example", pin).await);
        // Any other fingerprint.
        let mut wrong = pin;
        wrong[0] ^= 1;
        assert!(!dial_pinned(&server, "127.0.0.1", wrong).await);

        // Another cloud, with a certificate for the same address.
        let other_store = Store::in_memory().unwrap();
        let other_key = RootKey::load_or_create(&other_store).unwrap();
        let other = Ca::load_or_init(&other_store, &other_key).unwrap();
        let imposter = pki(
            &other,
            "pistack",
            vec!["127.0.0.1".into()],
            CertUsage::Server,
        );
        assert!(!dial_pinned(&imposter, "127.0.0.1", pin).await);
        // It cannot borrow the real root either: its leaf does not chain
        // to it.
        let mut borrowed = imposter.clone();
        borrowed.ca_pem = ca.bundle_pem();
        assert!(!dial_pinned(&borrowed, "127.0.0.1", pin).await);
        // A client certificate is no server certificate.
        let client = pki(&ca, "agent-7", vec!["127.0.0.1".into()], CertUsage::Client);
        assert!(!dial_pinned(&client, "127.0.0.1", pin).await);
    }

    #[test]
    fn the_trust_bundle_and_the_names_are_read_without_the_root_key() {
        let store = Store::in_memory().unwrap();
        assert_eq!(crate::ca::trust_bundle(&store).unwrap(), None);
        assert_eq!(crate::ca::root_fingerprint(&store).unwrap(), None);
        let key = RootKey::load_or_create(&store).unwrap();
        let ca = Ca::load_or_init(&store, &key).unwrap();
        assert_eq!(
            crate::ca::trust_bundle(&store).unwrap().as_deref(),
            Some(ca.bundle_pem().as_str())
        );
        let print = crate::ca::root_fingerprint(&store).unwrap().unwrap();
        let root = crate::ca::pem_to_der(&ca.root_pem()).unwrap();
        assert_eq!(print, crate::crypto::sha256_hex(&root));

        let sans = vec![
            "10.0.0.11".to_string(),
            "pistack".to_string(),
            "fd00::11".to_string(),
        ];
        let server = pki(&ca, "pistack", sans.clone(), CertUsage::Server);
        let der = crate::ca::pem_to_der(&server.cert_pem).unwrap();
        assert_eq!(crate::ca::names_from_der(&der).unwrap(), sans);
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

    #[tokio::test]
    async fn http_listener_accepts_ca_client_and_rejects_foreign() {
        let store = Store::in_memory().unwrap();
        let key = RootKey::load_or_create(&store).unwrap();
        let ca = Ca::load_or_init(&store, &key).unwrap();
        let server = pki(&ca, "gateway", vec!["127.0.0.1".into()], CertUsage::Server);
        let client = pki(&ca, "agent-http", vec![], CertUsage::Client);

        let acceptor = TlsAcceptor::from(Arc::new(server_config_http(&server).unwrap()));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let srv = tokio::spawn(async move {
            let mut outcomes = Vec::new();
            for _ in 0..3 {
                let (tcp, _) = listener.accept().await.unwrap();
                match acceptor.accept(tcp).await {
                    Ok(mut tls) => {
                        outcomes.push(peer_common_name(tls.get_ref().1));
                        tls.shutdown().await.ok();
                    }
                    Err(_) => outcomes.push(Some("handshake-failed".into())),
                }
            }
            outcomes
        });

        // CA-signed client cert.
        let connector = TlsConnector::from(Arc::new(
            client_config(&ca.bundle_pem(), Some(&client)).unwrap(),
        ));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
            .await
            .unwrap();
        tls.shutdown().await.ok();

        // No client cert (browser / bearer style).
        let connector =
            TlsConnector::from(Arc::new(client_config(&ca.bundle_pem(), None).unwrap()));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut tls = connector
            .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
            .await
            .unwrap();
        tls.shutdown().await.ok();

        // Foreign CA client cert: the server must reject during handshake.
        // The client may surface Ok or Err depending on when the alert arrives;
        // the server-side accept outcome is the authority.
        let other_store = Store::in_memory().unwrap();
        let other_key = RootKey::load_or_create(&other_store).unwrap();
        let other = Ca::load_or_init(&other_store, &other_key).unwrap();
        let foreign = pki(&other, "spy", vec![], CertUsage::Client);
        let connector = TlsConnector::from(Arc::new(
            client_config(&ca.bundle_pem(), Some(&foreign)).unwrap(),
        ));
        let tcp = tokio::net::TcpStream::connect(addr).await.unwrap();
        let foreign_client = connector
            .connect(ServerName::try_from("127.0.0.1").unwrap(), tcp)
            .await;
        if let Ok(mut tls) = foreign_client {
            // Drive the connection so the server finishes rejecting.
            let _ = tls.write_all(b"x").await;
            tls.shutdown().await.ok();
        }

        let outcomes = srv.await.unwrap();
        assert_eq!(outcomes[0].as_deref(), Some("agent-http"));
        assert_eq!(outcomes[1], None);
        assert_eq!(
            outcomes[2].as_deref(),
            Some("handshake-failed"),
            "foreign client cert must not authenticate: {outcomes:?}"
        );
    }
}
