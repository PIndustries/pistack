// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The cloud CA.
//!
//! On first start `keep` creates a self-signed root (`pistack-root`, ten
//! years) and an intermediate signed by it (`pistack-issuing`, five years).
//! Everything else is signed by the intermediate, so the root key is used
//! exactly once and could move offline later. Both CAs are
//! `CertificateAuthority` resources; their private keys are sealed with the
//! root key (`crypto.rs`) and stored as cloud-scoped `Secret`s.
//!
//! Serials come from one monotonic counter in store meta ([`META_SERIAL`]),
//! so no two certificates in the cloud share one. Revocation is a list of
//! serials on the CA's status, served as JSON by `GET /cas/{id}/crl`.

use chrono::{DateTime, Datelike, Duration, Utc};
use parking_lot::Mutex;
use pistack_store::Store;
use pistack_types::{Id, Phase};
use rcgen::{
    BasicConstraints, CertificateParams, DistinguishedName, DnType, ExtendedKeyUsagePurpose, IsCa,
    KeyPair, KeyUsagePurpose, SanType, SerialNumber, PKCS_ECDSA_P256_SHA256,
};
use std::net::IpAddr;
use std::sync::Arc;
use x509_parser::prelude::{FromDer, X509Certificate};

use crate::crypto::{sha256_hex, RootKey};
use crate::error::{KeepError, Result};
use crate::types::*;

pub const ROOT_NAME: &str = "pistack-root";
pub const ISSUING_NAME: &str = "pistack-issuing";
/// Store meta key: the last serial issued by any CA.
pub const META_SERIAL: &str = "keep/serial";
const ROOT_TTL_DAYS: u32 = 3650;
const ISSUING_TTL_DAYS: u32 = 1826;
/// Content type given to private-key secrets.
pub const PEM_CONTENT_TYPE: &str = "application/x-pem-file";
/// Label `keep` puts on secrets it manages itself; reserved for keep.
pub const ROLE_LABEL: &str = "keep.pistack.io/role";
/// [`ROLE_LABEL`] value on a certificate's or CA's private-key secret.
pub const ROLE_PRIVATE_KEY: &str = "private-key";

/// A freshly issued certificate and its key. The key PEM exists only here
/// until the caller seals it.
#[derive(Clone, Debug)]
pub struct Issued {
    pub cert_pem: String,
    pub key_pem: String,
    pub serial: u64,
    pub not_before: DateTime<Utc>,
    pub not_after: DateTime<Utc>,
    pub fingerprint_sha256: String,
    pub issuer: Id,
}

/// One CA we can sign with.
struct Signer {
    id: Id,
    name: String,
    /// Issuer handle for rcgen: carries DN, key-id method and key usages.
    handle: rcgen::Certificate,
    /// The real certificate as persisted and served.
    pem: String,
    der: Vec<u8>,
    key: KeyPair,
    not_after: DateTime<Utc>,
}

/// The CA. Cheap to clone; signers are shared.
#[derive(Clone)]
pub struct Ca {
    store: Store,
    signers: Arc<Mutex<Vec<Signer>>>,
    root_id: Id,
    issuing_id: Id,
}

impl Ca {
    /// Load the root and issuing CAs, creating them on first start.
    pub fn load_or_init(store: &Store, root_key: &RootKey) -> Result<Self> {
        let cas = store.collection::<CertificateAuthority>();
        let root = match try_by_name(store, ROOT_NAME)? {
            Some(r) => r,
            None => {
                let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
                let serial = next_serial(store)?;
                let params = ca_params(ROOT_NAME, ROOT_TTL_DAYS, serial, true);
                let cert = params.self_signed(&key)?;
                let ca = persist_ca(
                    store,
                    root_key,
                    ROOT_NAME,
                    ROOT_TTL_DAYS,
                    None,
                    &cert,
                    &key,
                    serial,
                )?;
                tracing::info!(ca = ROOT_NAME, "keep: created root CA");
                ca
            }
        };
        let issuing = match try_by_name(store, ISSUING_NAME)? {
            Some(i) => i,
            None => {
                let root_signer = load_signer(store, root_key, &root)?;
                let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
                let serial = next_serial(store)?;
                let params = ca_params(ISSUING_NAME, ISSUING_TTL_DAYS, serial, false);
                let cert = params.signed_by(&key, &root_signer.handle, &root_signer.key)?;
                let ca = persist_ca(
                    store,
                    root_key,
                    ISSUING_NAME,
                    ISSUING_TTL_DAYS,
                    Some(root.meta.id.clone()),
                    &cert,
                    &key,
                    serial,
                )?;
                tracing::info!(ca = ISSUING_NAME, "keep: created issuing CA");
                ca
            }
        };
        let signers = cas
            .list(None)?
            .iter()
            .map(|c| load_signer(store, root_key, c))
            .collect::<Result<Vec<_>>>()?;
        Ok(Self {
            store: store.clone(),
            signers: Arc::new(Mutex::new(signers)),
            root_id: root.meta.id,
            issuing_id: issuing.meta.id,
        })
    }

    pub fn root_id(&self) -> &Id {
        &self.root_id
    }
    pub fn issuing_id(&self) -> &Id {
        &self.issuing_id
    }

    fn with_signer<T>(&self, id: &Id, f: impl FnOnce(&Signer) -> Result<T>) -> Result<T> {
        let signers = self.signers.lock();
        let s = signers
            .iter()
            .find(|s| &s.id == id)
            .ok_or_else(|| KeepError::NotFound(format!("ca {id} not found")))?;
        f(s)
    }

    pub fn root_pem(&self) -> String {
        self.with_signer(&self.root_id, |s| Ok(s.pem.clone()))
            .unwrap_or_default()
    }
    pub fn issuing_pem(&self) -> String {
        self.with_signer(&self.issuing_id, |s| Ok(s.pem.clone()))
            .unwrap_or_default()
    }
    /// Issuing CA followed by the root: what clients trust.
    pub fn bundle_pem(&self) -> String {
        let mut b = self.issuing_pem();
        if !b.ends_with('\n') {
            b.push('\n');
        }
        b.push_str(&self.root_pem());
        b
    }

    /// Issue a leaf certificate. The caller persists the result.
    pub fn issue(&self, spec: &CertificateSpec) -> Result<Issued> {
        if spec.common_name.trim().is_empty() {
            return Err(KeepError::Invalid("common_name is required".into()));
        }
        if spec.ttl_days == 0 {
            return Err(KeepError::Invalid("ttl_days must be at least 1".into()));
        }
        let issuer = spec
            .issuer
            .clone()
            .unwrap_or_else(|| self.issuing_id.clone());
        let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256)?;
        let serial = next_serial(&self.store)?;

        let mut params = CertificateParams::default();
        params.distinguished_name = dn(&spec.common_name);
        params.subject_alt_names = spec
            .sans
            .iter()
            .map(|s| san(s))
            .collect::<Result<Vec<_>>>()?;
        params.is_ca = IsCa::ExplicitNoCa;
        params.key_usages = vec![KeyUsagePurpose::DigitalSignature];
        params.extended_key_usages = match spec.usage {
            CertUsage::Server => vec![ExtendedKeyUsagePurpose::ServerAuth],
            CertUsage::Client => vec![ExtendedKeyUsagePurpose::ClientAuth],
            CertUsage::Both => vec![
                ExtendedKeyUsagePurpose::ServerAuth,
                ExtendedKeyUsagePurpose::ClientAuth,
            ],
        };
        params.use_authority_key_identifier_extension = true;
        params.serial_number = Some(SerialNumber::from_slice(&serial.to_be_bytes()));
        let today = Utc::now().date_naive();
        let (y, m, d) = ymd(today - Duration::days(1));
        params.not_before = rcgen::date_time_ymd(y, m, d);

        let cert = self.with_signer(&issuer, |s| {
            let mut end = today + Duration::days(i64::from(spec.ttl_days));
            if ymd_to_utc(end) > s.not_after {
                end = s.not_after.date_naive();
            }
            let (y, m, d) = ymd(end);
            params.not_after = rcgen::date_time_ymd(y, m, d);
            Ok(params.signed_by(&key, &s.handle, &s.key)?)
        })?;
        let (not_before, not_after) = validity(cert.der())?;
        let fingerprint_sha256 = sha256_hex(cert.der());

        let cas = self.store.collection::<CertificateAuthority>();
        cas.modify(&issuer, |c| {
            c.status.serial = serial;
            Ok(())
        })?;
        Ok(Issued {
            cert_pem: cert.pem(),
            key_pem: key.serialize_pem(),
            serial,
            not_before,
            not_after,
            fingerprint_sha256,
            issuer,
        })
    }

    /// Mark `serial` revoked on CA `ca`. Idempotent.
    pub fn revoke(&self, ca: &Id, serial: u64) -> Result<CertificateAuthority> {
        let cas = self.store.collection::<CertificateAuthority>();
        Ok(cas.modify(ca, |c| {
            if !c.status.revoked.iter().any(|r| r.serial == serial) {
                c.status.revoked.push(Revocation {
                    serial,
                    revoked_at: Utc::now(),
                });
            }
            Ok(())
        })?)
    }

    /// Revoked serials of CA `ca`.
    pub fn crl(&self, ca: &Id) -> Result<Vec<Revocation>> {
        Ok(self
            .store
            .collection::<CertificateAuthority>()
            .get(ca)?
            .status
            .revoked)
    }

    /// Check a DER client certificate: signed by one of our CAs, within its
    /// validity window, not revoked, and (if it carries an EKU) allowed for
    /// client auth. Returns the subject common name.
    pub fn verify_client_cert(&self, der: &[u8]) -> Result<String> {
        let (_, cert) = X509Certificate::from_der(der)
            .map_err(|e| KeepError::Pki(format!("cannot parse certificate: {e}")))?;
        if !cert.validity().is_valid() {
            return Err(KeepError::Unauthorized(
                "certificate is not valid now".into(),
            ));
        }
        if let Ok(Some(eku)) = cert.extended_key_usage() {
            if !eku.value.client_auth && !eku.value.any {
                return Err(KeepError::Unauthorized(
                    "certificate is not for client auth".into(),
                ));
            }
        }
        let issuer_dn = cert.issuer().to_string();
        let (ca_id, ca_der) = {
            let signers = self.signers.lock();
            let s = signers
                .iter()
                .find(|s| {
                    X509Certificate::from_der(&s.der)
                        .map(|(_, c)| c.subject().to_string() == issuer_dn)
                        .unwrap_or(false)
                })
                .ok_or_else(|| KeepError::Unauthorized("unknown issuer".into()))?;
            (s.id.clone(), s.der.clone())
        };
        let (_, ca_cert) = X509Certificate::from_der(&ca_der)
            .map_err(|e| KeepError::Pki(format!("cannot parse CA certificate: {e}")))?;
        cert.verify_signature(Some(ca_cert.public_key()))
            .map_err(|_| KeepError::Unauthorized("bad certificate signature".into()))?;
        let serial = serial_u64(cert.serial.to_bytes_be().as_slice());
        if self.crl(&ca_id)?.iter().any(|r| r.serial == serial) {
            return Err(KeepError::Unauthorized("certificate is revoked".into()));
        }
        common_name(&cert)
    }
}

// ---- helpers -----------------------------------------------------------------

fn try_by_name(store: &Store, name: &str) -> Result<Option<CertificateAuthority>> {
    match store
        .collection::<CertificateAuthority>()
        .get_by_name(None, name)
    {
        Ok(c) => Ok(Some(c)),
        Err(pistack_store::StoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}

/// Next value of the cloud-wide serial counter. The store's write lock does
/// not cover read-modify-write on meta, so this takes a process-wide lock.
fn next_serial(store: &Store) -> Result<u64> {
    static LOCK: Mutex<()> = Mutex::new(());
    let _g = LOCK.lock();
    let next = store.get_meta::<u64>(META_SERIAL)?.unwrap_or(0) + 1;
    store.put_meta(META_SERIAL, &next)?;
    Ok(next)
}

fn dn(cn: &str) -> DistinguishedName {
    let mut d = DistinguishedName::new();
    d.push(DnType::OrganizationName, "PiStack");
    d.push(DnType::CommonName, cn);
    d
}

fn san(s: &str) -> Result<SanType> {
    Ok(match s.parse::<IpAddr>() {
        Ok(ip) => SanType::IpAddress(ip),
        Err(_) => SanType::DnsName(
            s.try_into()
                .map_err(|_| KeepError::Invalid(format!("'{s}' is not a DNS name or IP")))?,
        ),
    })
}

/// `(year, month, day)` for `rcgen::date_time_ymd`; rcgen's `time` crate is
/// not a direct dependency, so dates are passed at day granularity.
fn ymd(d: chrono::NaiveDate) -> (i32, u8, u8) {
    (d.year(), d.month() as u8, d.day() as u8)
}

fn ymd_to_utc(d: chrono::NaiveDate) -> DateTime<Utc> {
    d.and_hms_opt(0, 0, 0)
        .map(|t| t.and_utc())
        .unwrap_or_else(Utc::now)
}

/// Parameters shared by both CA certificates. The handle rebuilt at load
/// time (`issuer_handle`) must agree with these on DN, key-id method and
/// key usages, which is what `signed_by` reads from the issuer.
fn ca_params(cn: &str, ttl_days: u32, serial: u64, root: bool) -> CertificateParams {
    let mut p = CertificateParams::default();
    p.distinguished_name = dn(cn);
    p.is_ca = if root {
        IsCa::Ca(BasicConstraints::Unconstrained)
    } else {
        IsCa::Ca(BasicConstraints::Constrained(0))
    };
    p.key_usages = vec![
        KeyUsagePurpose::KeyCertSign,
        KeyUsagePurpose::CrlSign,
        KeyUsagePurpose::DigitalSignature,
    ];
    p.use_authority_key_identifier_extension = !root;
    p.serial_number = Some(SerialNumber::from_slice(&serial.to_be_bytes()));
    let today = Utc::now().date_naive();
    let (y, m, d) = ymd(today - Duration::days(1));
    p.not_before = rcgen::date_time_ymd(y, m, d);
    let (y, m, d) = ymd(today + Duration::days(i64::from(ttl_days)));
    p.not_after = rcgen::date_time_ymd(y, m, d);
    p
}

/// Rebuild the rcgen issuer handle for a CA from its name and key. Only the
/// DN, key-identifier method and key usages matter for signing, and those
/// are fixed by `ca_params`; the DER this produces is never served.
fn issuer_handle(cn: &str, root: bool, key: &KeyPair) -> Result<rcgen::Certificate> {
    Ok(ca_params(cn, 1, 1, root).self_signed(key)?)
}

#[allow(clippy::too_many_arguments)]
fn persist_ca(
    store: &Store,
    root_key: &RootKey,
    name: &str,
    ttl_days: u32,
    parent: Option<Id>,
    cert: &rcgen::Certificate,
    key: &KeyPair,
    serial: u64,
) -> Result<CertificateAuthority> {
    let key_secret = seal_key_secret(store, root_key, &format!("{name}-key"), None, key)?;
    let (not_before, not_after) = validity(cert.der())?;
    let mut ca = CertificateAuthority::new(
        name,
        None,
        CaSpec {
            common_name: name.to_owned(),
            ttl_days,
            parent,
        },
    );
    ca.meta.description = "PiStack cloud CA".to_owned();
    ca.status = CaStatus {
        phase: Phase::Ready,
        cert_pem: cert.pem(),
        fingerprint_sha256: sha256_hex(cert.der()),
        not_before: Some(not_before),
        not_after: Some(not_after),
        serial,
        key_secret: Some(key_secret),
        revoked: Vec::new(),
    };
    Ok(store.collection::<CertificateAuthority>().create(ca)?)
}

/// Store a private key as an encrypted, cloud- or project-scoped Secret.
pub(crate) fn seal_key_secret(
    store: &Store,
    root_key: &RootKey,
    name: &str,
    project: Option<Id>,
    key: &KeyPair,
) -> Result<Id> {
    seal_pem_secret(store, root_key, name, project, &key.serialize_pem())
}

pub(crate) fn seal_pem_secret(
    store: &Store,
    root_key: &RootKey,
    name: &str,
    project: Option<Id>,
    pem: &str,
) -> Result<Id> {
    let mut s = StoredSecret::new(
        name,
        project,
        SecretSpec {
            kind: SecretKind::Certificate,
            content_type: PEM_CONTENT_TYPE.to_owned(),
            expires_at: None,
            rotate_after_days: None,
        },
    );
    s.meta
        .labels
        .insert(ROLE_LABEL.to_owned(), ROLE_PRIVATE_KEY.to_owned());
    let aad = s.meta.id.to_string();
    s.status.sealed = Some(root_key.seal(pem.as_bytes(), aad.as_bytes())?);
    s.status.public = SecretStatus {
        phase: Phase::Ready,
        version: 1,
        last_rotated: None,
    };
    Ok(store.collection::<StoredSecret>().create(s)?.meta.id)
}

/// Open a sealed secret's payload.
pub(crate) fn open_secret(store: &Store, root_key: &RootKey, id: &Id) -> Result<Vec<u8>> {
    let s = store.collection::<StoredSecret>().get(id)?;
    let sealed = s
        .status
        .sealed
        .as_ref()
        .ok_or_else(|| KeepError::NotFound(format!("secret {id} has no payload")))?;
    root_key.open(sealed, id.to_string().as_bytes())
}

fn load_signer(store: &Store, root_key: &RootKey, ca: &CertificateAuthority) -> Result<Signer> {
    let key_id = ca
        .status
        .key_secret
        .as_ref()
        .ok_or_else(|| KeepError::Pki(format!("ca {} has no key", ca.meta.name)))?;
    let key_pem = open_secret(store, root_key, key_id)?;
    let key_pem =
        String::from_utf8(key_pem).map_err(|_| KeepError::Pki("ca key is not PEM text".into()))?;
    let key = KeyPair::from_pem(&key_pem)?;
    let der = pem_to_der(&ca.status.cert_pem)?;
    let (_, not_after) = validity(&der)?;
    let handle = issuer_handle(&ca.spec.common_name, ca.spec.parent.is_none(), &key)?;
    Ok(Signer {
        id: ca.meta.id.clone(),
        name: ca.meta.name.clone(),
        handle,
        pem: ca.status.cert_pem.clone(),
        der,
        key,
        not_after,
    })
}

/// First certificate in a PEM string, as DER.
pub fn pem_to_der(pem_str: &str) -> Result<Vec<u8>> {
    let p = pem::parse(pem_str).map_err(|e| KeepError::Pki(format!("bad PEM: {e}")))?;
    if p.tag() != "CERTIFICATE" {
        return Err(KeepError::Pki(format!(
            "expected CERTIFICATE, got {}",
            p.tag()
        )));
    }
    Ok(p.into_contents())
}

/// `(not_before, not_after)` of a DER certificate.
pub fn validity(der: &[u8]) -> Result<(DateTime<Utc>, DateTime<Utc>)> {
    let (_, cert) = X509Certificate::from_der(der)
        .map_err(|e| KeepError::Pki(format!("cannot parse certificate: {e}")))?;
    let v = cert.validity();
    let nb = DateTime::<Utc>::from_timestamp(v.not_before.timestamp(), 0)
        .ok_or_else(|| KeepError::Pki("not_before out of range".into()))?;
    let na = DateTime::<Utc>::from_timestamp(v.not_after.timestamp(), 0)
        .ok_or_else(|| KeepError::Pki("not_after out of range".into()))?;
    Ok((nb, na))
}

/// Subject common name of a DER certificate.
pub fn common_name_from_der(der: &[u8]) -> Result<String> {
    let (_, cert) = X509Certificate::from_der(der)
        .map_err(|e| KeepError::Pki(format!("cannot parse certificate: {e}")))?;
    common_name(&cert)
}

fn common_name(cert: &X509Certificate<'_>) -> Result<String> {
    cert.subject()
        .iter_common_name()
        .next()
        .and_then(|a| a.as_str().ok())
        .map(str::to_owned)
        .ok_or_else(|| KeepError::Pki("certificate has no common name".into()))
}

fn serial_u64(be: &[u8]) -> u64 {
    let mut out = 0u64;
    for b in be.iter().skip(be.len().saturating_sub(8)) {
        out = (out << 8) | u64::from(*b);
    }
    out
}

impl std::fmt::Debug for Signer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Signer({})", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustls::client::WebPkiServerVerifier;
    use rustls::pki_types::{CertificateDer, ServerName, UnixTime};
    use rustls::RootCertStore;

    fn boot() -> (Store, Ca) {
        let store = Store::in_memory().unwrap();
        let key = RootKey::load_or_create(&store).unwrap();
        let ca = Ca::load_or_init(&store, &key).unwrap();
        (store, ca)
    }

    fn der(pem_str: &str) -> CertificateDer<'static> {
        CertificateDer::from(pem_to_der(pem_str).unwrap())
    }

    #[test]
    fn init_creates_root_and_issuing_once() {
        let (store, ca) = boot();
        let cas = store.collection::<CertificateAuthority>();
        assert_eq!(cas.list(None).unwrap().len(), 2);
        let root = cas.get(ca.root_id()).unwrap();
        let issuing = cas.get(ca.issuing_id()).unwrap();
        assert_eq!(root.meta.name, ROOT_NAME);
        assert_eq!(issuing.spec.parent.as_ref(), Some(ca.root_id()));
        assert!(root.status.cert_pem.contains("BEGIN CERTIFICATE"));
        // Keys exist as sealed secrets and never in the clear.
        let secrets = store.collection::<StoredSecret>().list(None).unwrap();
        assert_eq!(secrets.len(), 2);
        assert!(secrets.iter().all(|s| s.status.sealed.is_some()));
        let raw = serde_json::to_string(&secrets).unwrap();
        assert!(!raw.contains("PRIVATE KEY"));
        // Second load reuses them.
        let again = Ca::load_or_init(&store, &RootKey::load_or_create(&store).unwrap()).unwrap();
        assert_eq!(cas.list(None).unwrap().len(), 2);
        assert_eq!(again.root_pem(), ca.root_pem());
        assert_eq!(
            ca.bundle_pem(),
            format!("{}{}", ca.issuing_pem(), ca.root_pem())
        );
    }

    #[test]
    fn issued_server_cert_chains_to_root() {
        let (_store, ca) = boot();
        let issued = ca
            .issue(&CertificateSpec {
                common_name: "api.pistack.local".into(),
                sans: vec!["api.pistack.local".into(), "127.0.0.1".into()],
                usage: CertUsage::Server,
                ttl_days: 30,
                issuer: None,
            })
            .unwrap();
        assert!(issued.key_pem.contains("PRIVATE KEY"));
        assert_eq!(issued.serial, 3);
        assert!(issued.not_after > issued.not_before);
        assert_eq!(issued.fingerprint_sha256.len(), 64);

        let mut roots = RootCertStore::empty();
        roots.add(der(&ca.root_pem())).unwrap();
        let verifier = WebPkiServerVerifier::builder_with_provider(
            Arc::new(roots),
            Arc::new(rustls::crypto::aws_lc_rs::default_provider()),
        )
        .build()
        .unwrap();
        use rustls::client::danger::ServerCertVerifier;
        let leaf = der(&issued.cert_pem);
        let inter = [der(&ca.issuing_pem())];
        for name in ["api.pistack.local", "127.0.0.1"] {
            verifier
                .verify_server_cert(
                    &leaf,
                    &inter,
                    &ServerName::try_from(name).unwrap(),
                    &[],
                    UnixTime::now(),
                )
                .unwrap();
        }
        assert!(verifier
            .verify_server_cert(
                &leaf,
                &inter,
                &ServerName::try_from("other.example").unwrap(),
                &[],
                UnixTime::now(),
            )
            .is_err());
        // Without the intermediate the chain is incomplete.
        assert!(verifier
            .verify_server_cert(
                &leaf,
                &[],
                &ServerName::try_from("api.pistack.local").unwrap(),
                &[],
                UnixTime::now(),
            )
            .is_err());
    }

    #[test]
    fn client_cert_verifies_and_revokes() {
        let (_store, ca) = boot();
        let issued = ca
            .issue(&CertificateSpec {
                common_name: "agent-01".into(),
                usage: CertUsage::Client,
                ..Default::default()
            })
            .unwrap();
        let der = pem_to_der(&issued.cert_pem).unwrap();
        assert_eq!(ca.verify_client_cert(&der).unwrap(), "agent-01");

        // A server-only cert is refused for client auth.
        let server = ca
            .issue(&CertificateSpec {
                common_name: "web".into(),
                usage: CertUsage::Server,
                ..Default::default()
            })
            .unwrap();
        assert!(ca
            .verify_client_cert(&pem_to_der(&server.cert_pem).unwrap())
            .is_err());

        // A cert from a foreign CA is refused.
        let (_s2, other) = boot();
        let foreign = other
            .issue(&CertificateSpec {
                common_name: "agent-01".into(),
                usage: CertUsage::Client,
                ..Default::default()
            })
            .unwrap();
        assert!(ca
            .verify_client_cert(&pem_to_der(&foreign.cert_pem).unwrap())
            .is_err());

        // Revocation.
        ca.revoke(ca.issuing_id(), issued.serial).unwrap();
        ca.revoke(ca.issuing_id(), issued.serial).unwrap();
        let crl = ca.crl(ca.issuing_id()).unwrap();
        assert_eq!(crl.len(), 1);
        assert_eq!(crl[0].serial, issued.serial);
        assert!(matches!(
            ca.verify_client_cert(&der),
            Err(KeepError::Unauthorized(_))
        ));
    }

    #[test]
    fn ttl_is_clamped_to_issuer_and_validated() {
        let (_store, ca) = boot();
        let issued = ca
            .issue(&CertificateSpec {
                common_name: "long".into(),
                ttl_days: 36500,
                ..Default::default()
            })
            .unwrap();
        let (_, issuing_na) = validity(&pem_to_der(&ca.issuing_pem()).unwrap()).unwrap();
        assert!(issued.not_after <= issuing_na);
        assert!(matches!(
            ca.issue(&CertificateSpec::default()),
            Err(KeepError::Invalid(_))
        ));
        assert!(ca
            .issue(&CertificateSpec {
                common_name: "x".into(),
                issuer: Some(Id::new(KIND_CA)),
                ..Default::default()
            })
            .is_err());
    }
}
