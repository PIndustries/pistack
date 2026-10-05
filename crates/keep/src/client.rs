// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `keep::Client`: what the binary and other modules use. Everything the
//! HTTP layer does goes through here too, so the two never disagree.

use chrono::Utc;
use pistack_store::Store;
use pistack_types::{Id, Phase};
use std::collections::BTreeMap;

use crate::access_key::{ACCESS_KEYS, ROLE_ACCESS_KEY};
use crate::backend::KeyBackend;
use crate::ca::{
    open_secret, seal_pem_secret, Ca, Issued, RESERVED_LABEL_PREFIX, ROLE_LABEL, ROLE_PRIVATE_KEY,
};
use crate::crypto::RootKey;
use crate::error::{KeepError, Result};
use crate::quota::{self, KeepConfig, CERTIFICATES, SECRETS};
use crate::tls::Pki;
use crate::types::*;

/// Largest secret payload accepted (1 MiB).
pub const MAX_PAYLOAD: usize = 1 << 20;
/// Limits on a secret's clear-text metadata, so labels, description and
/// content type cannot carry more than the (sealed) payload is allowed to.
/// Applied the same way on create and update, and to certificates.
pub const MAX_LABELS: usize = 64;
pub const MAX_LABEL_KEY: usize = 128;
pub const MAX_LABEL_VALUE: usize = 1024;
pub const MAX_DESCRIPTION: usize = 4096;
pub const MAX_CONTENT_TYPE: usize = 255;

#[derive(Clone)]
pub struct Client {
    store: Store,
    root_key: RootKey,
    ca: Ca,
    limits: KeepConfig,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("keep::Client")
    }
}

impl Client {
    /// Open (or on first start, initialise) the root key through the
    /// process's configured backend, and the cloud CA, on `store`.
    pub fn open(store: &Store) -> Result<Self> {
        Self::open_with(store, crate::backend::configured()?)
    }

    /// [`open`](Self::open) with an explicit root-key backend.
    pub fn open_with(store: &Store, backend: &KeyBackend) -> Result<Self> {
        let root_key = backend.load_or_create(store)?;
        let ca = Ca::load_or_init(store, &root_key)?;
        Ok(Self {
            store: store.clone(),
            root_key,
            ca,
            limits: KeepConfig::configured().clone(),
        })
    }

    /// Replace the count limits (normally `[modules.keep]`).
    pub fn with_limits(mut self, limits: KeepConfig) -> Self {
        self.limits = limits;
        self
    }

    pub fn limits(&self) -> &KeepConfig {
        &self.limits
    }

    /// Rows of `resource` in `project` that count against its quota. A
    /// certificate's key secret counts as part of the certificate; an
    /// access key counts under `access_keys`, not `secrets`.
    pub(crate) fn count(&self, project: &Id, resource: &str) -> Result<u64> {
        let n = if resource == CERTIFICATES {
            self.store
                .collection::<Certificate>()
                .list(Some(project))?
                .len()
        } else {
            let want = (resource == ACCESS_KEYS).then_some(ROLE_ACCESS_KEY);
            self.store
                .collection::<StoredSecret>()
                .list(Some(project))?
                .iter()
                .filter(|s| managed_role(&s.meta.labels) == want)
                .count()
        };
        Ok(n as u64)
    }

    /// Refuse one more `resource` in `project` at its limit. Call with
    /// [`quota::admission`] held when a create follows.
    pub(crate) fn admit(&self, project: Option<&Id>, resource: &str) -> Result<()> {
        let Some(p) = project else {
            return Ok(());
        };
        let default = match resource {
            CERTIFICATES => self.limits.max_certificates_per_project,
            ACCESS_KEYS => self.limits.max_access_keys_per_project,
            _ => self.limits.max_secrets_per_project,
        };
        quota::admit(&self.store, p, resource, self.count(p, resource)?, default)
    }

    /// Mirror the current count into the project's usage.
    pub(crate) fn note_usage(&self, project: Option<&Id>, resource: &str) {
        let Some(p) = project else {
            return;
        };
        match self.count(p, resource) {
            Ok(n) => quota::record(&self.store, p, resource, n),
            Err(e) => {
                tracing::warn!(project = %p, resource, error = %e, "keep: usage was not counted")
            }
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn ca(&self) -> &Ca {
        &self.ca
    }

    // ---- PKI for the binary ------------------------------------------------

    /// Server certificate for a gateway or agent endpoint. `sans` are DNS
    /// names and IPs; the common name is not implied as a SAN.
    pub fn issue_server_cert(&self, cn: &str, sans: &[String]) -> Result<Pki> {
        self.issue_pki(cn, sans.to_vec(), CertUsage::Server)
    }

    /// Client certificate an agent or service presents for mTLS.
    pub fn issue_client_cert(&self, cn: &str) -> Result<Pki> {
        self.issue_pki(cn, Vec::new(), CertUsage::Client)
    }

    fn issue_pki(&self, cn: &str, sans: Vec<String>, usage: CertUsage) -> Result<Pki> {
        let spec = CertificateSpec {
            common_name: cn.to_owned(),
            sans,
            usage,
            ttl_days: default_ttl_days(),
            issuer: None,
        };
        let issued = self.ca.issue(&spec)?;
        let name = format!("{}-{}", sanitize(cn), issued.serial);
        let pki = Pki {
            cert_pem: issued.cert_pem.clone(),
            key_pem: issued.key_pem.clone(),
            ca_pem: self.ca.bundle_pem(),
        };
        self.persist_certificate(None, &name, spec, issued, String::new(), BTreeMap::new())?;
        Ok(pki)
    }

    /// Issuing CA then root: what every client should trust.
    pub fn ca_bundle_pem(&self) -> String {
        self.ca.bundle_pem()
    }

    /// Verify a DER client certificate against the cloud CA; returns its CN.
    pub fn verify_client_cert(&self, der: &[u8]) -> Result<String> {
        self.ca.verify_client_cert(der)
    }

    // ---- secrets -----------------------------------------------------------

    /// Decrypted payload of a secret. An access key's secret is refused:
    /// it is shown once at creation and otherwise only reaches
    /// [`lookup_access_key`](Self::lookup_access_key).
    pub fn get_secret_payload(&self, id: &Id) -> Result<Vec<u8>> {
        let s = self.store.collection::<Secret>().get(id)?;
        if managed_role(&s.meta.labels) == Some(ROLE_ACCESS_KEY) {
            return Err(KeepError::Conflict(
                "an access key's secret is shown only once, when it is created".into(),
            ));
        }
        open_secret(&self.store, &self.root_key, id)
    }

    pub(crate) fn root_key(&self) -> &RootKey {
        &self.root_key
    }

    pub fn get_secret(&self, id: &Id) -> Result<Secret> {
        Ok(self.store.collection::<Secret>().get(id)?)
    }

    pub fn list_secrets(&self, project: Option<&Id>) -> Result<Vec<Secret>> {
        Ok(self.store.collection::<Secret>().list(project)?)
    }

    /// Create a secret with its first payload version.
    pub fn create_secret(&self, mut secret: Secret, payload: &[u8]) -> Result<Secret> {
        check_payload(payload)?;
        check_metadata(&secret)?;
        if let Some(k) = secret
            .meta
            .labels
            .keys()
            .find(|k| k.starts_with(RESERVED_LABEL_PREFIX))
        {
            return Err(KeepError::Invalid(format!(
                "label {k} is reserved for keep ({RESERVED_LABEL_PREFIX}*)"
            )));
        }
        secret.status = SecretStatus {
            phase: Phase::Ready,
            version: 1,
            last_rotated: None,
        };
        let mut stored = StoredSecret {
            meta: secret.meta,
            spec: secret.spec,
            status: StoredSecretStatus {
                public: secret.status,
                sealed: None,
            },
        };
        let aad = stored.meta.id.to_string();
        stored.status.sealed = Some(self.root_key.seal(payload, aad.as_bytes())?);
        let project = stored.meta.project.clone();
        let created = {
            let _admission = quota::admission();
            self.admit(project.as_ref(), SECRETS)?;
            self.store.collection::<StoredSecret>().create(stored)?
        };
        self.note_usage(project.as_ref(), SECRETS);
        Ok(public_secret(created))
    }

    /// Replace spec and metadata (not the payload). Honours `If-Match`.
    /// The same metadata limits as create apply, and keep's own labels
    /// (`keep.pistack.io/*`) cannot be added, changed or removed.
    pub fn update_secret(&self, secret: Secret, expected_revision: Option<u64>) -> Result<Secret> {
        check_metadata(&secret)?;
        let col = self.store.collection::<StoredSecret>();
        let current = col.get(&secret.meta.id)?;
        if reserved_labels(&secret.meta.labels) != reserved_labels(&current.meta.labels) {
            return Err(KeepError::Conflict(format!(
                "labels {RESERVED_LABEL_PREFIX}* are managed by keep and cannot be changed \
                 (including {ROLE_LABEL})"
            )));
        }
        let stored = StoredSecret {
            meta: secret.meta,
            spec: secret.spec,
            status: StoredSecretStatus {
                public: current.status.public,
                sealed: current.status.sealed,
            },
        };
        Ok(public_secret(col.update(stored, expected_revision)?))
    }

    /// Rotate: new payload, version + 1.
    pub fn rotate_secret(
        &self,
        id: &Id,
        payload: &[u8],
        expected_revision: Option<u64>,
    ) -> Result<Secret> {
        check_payload(payload)?;
        let col = self.store.collection::<StoredSecret>();
        let mut cur = col.get(id)?;
        match managed_role(&cur.meta.labels) {
            Some(ROLE_PRIVATE_KEY) => {
                return Err(KeepError::Conflict(
                    "this secret holds a certificate's key; it cannot be rotated on its own".into(),
                ))
            }
            Some(ROLE_ACCESS_KEY) => {
                return Err(KeepError::Conflict(
                    "an access key cannot be rotated; create a new one and revoke this one".into(),
                ))
            }
            Some(_) => {
                return Err(KeepError::Conflict(
                    "this secret is managed by keep and cannot be rotated".into(),
                ))
            }
            None => {}
        }
        if let Some(exp) = expected_revision {
            if exp != cur.meta.revision {
                return Err(pistack_store::StoreError::Revision {
                    expected: exp,
                    found: cur.meta.revision,
                }
                .into());
            }
        }
        let aad = id.to_string();
        cur.status.sealed = Some(self.root_key.seal(payload, aad.as_bytes())?);
        cur.status.public.version += 1;
        cur.status.public.last_rotated = Some(Utc::now());
        cur.status.public.phase = Phase::Ready;
        let rev = cur.meta.revision;
        Ok(public_secret(col.update(cur, Some(rev))?))
    }

    /// Delete a secret. Deleting an access key's secret revokes the key.
    pub fn delete_secret(&self, id: &Id) -> Result<Secret> {
        let s = self.store.collection::<Secret>().get(id)?;
        if managed_role(&s.meta.labels) == Some(ROLE_ACCESS_KEY) {
            self.revoke_access_key(id)?;
            return Ok(s);
        }
        let gone = self.store.collection::<StoredSecret>().delete(id)?;
        self.note_usage(gone.meta.project.as_ref(), SECRETS);
        Ok(public_secret(gone))
    }

    // ---- certificates ------------------------------------------------------

    /// Issue and persist a certificate; its key becomes a Secret named
    /// `<name>-key` in the same project.
    pub fn issue_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
    ) -> Result<Certificate> {
        self.issue_certificate_with(project, name, spec, String::new(), BTreeMap::new())
    }

    /// [`issue_certificate`](Self::issue_certificate) with a description
    /// and labels, held to the same limits as a secret's. Nothing is issued
    /// when they, or the project's `certificates` quota, refuse.
    pub fn issue_certificate_with(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        description: String,
        labels: BTreeMap<String, String>,
    ) -> Result<Certificate> {
        check_meta_fields(&labels, &description)?;
        // Early refusal so a full project does not spend a serial; the
        // authoritative check is repeated under the lock in persist.
        self.admit(project.as_ref(), CERTIFICATES)?;
        let issued = self.ca.issue(&spec)?;
        self.persist_certificate(project, name, spec, issued, description, labels)
    }

    fn persist_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        issued: Issued,
        description: String,
        labels: BTreeMap<String, String>,
    ) -> Result<Certificate> {
        let created = {
            let _admission = quota::admission();
            self.admit(project.as_ref(), CERTIFICATES)?;
            self.store_certificate(project.clone(), name, spec, issued, description, labels)?
        };
        self.note_usage(project.as_ref(), CERTIFICATES);
        Ok(created)
    }

    fn store_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        issued: Issued,
        description: String,
        labels: BTreeMap<String, String>,
    ) -> Result<Certificate> {
        let key_secret = seal_pem_secret(
            &self.store,
            &self.root_key,
            &format!("{name}-key"),
            project.clone(),
            &issued.key_pem,
        )?;
        let mut cert = Certificate::new(name, project, spec);
        cert.meta.description = description;
        cert.meta.labels = labels;
        cert.status = CertificateStatus {
            phase: Phase::Ready,
            serial: issued.serial,
            not_before: Some(issued.not_before),
            not_after: Some(issued.not_after),
            fingerprint_sha256: issued.fingerprint_sha256,
            cert_pem: issued.cert_pem,
            issuer: Some(issued.issuer),
            key_secret: Some(key_secret.clone()),
            revoked: false,
        };
        match self.store.collection::<Certificate>().create(cert) {
            Ok(c) => Ok(c),
            Err(e) => {
                // Do not leave an orphaned key behind.
                let _ = self.store.collection::<StoredSecret>().delete(&key_secret);
                Err(e.into())
            }
        }
    }

    pub fn get_certificate(&self, id: &Id) -> Result<Certificate> {
        Ok(self.store.collection::<Certificate>().get(id)?)
    }

    pub fn list_certificates(&self, project: Option<&Id>) -> Result<Vec<Certificate>> {
        Ok(self.store.collection::<Certificate>().list(project)?)
    }

    /// Decrypted private key PEM of a certificate.
    pub fn certificate_key_pem(&self, cert: &Certificate) -> Result<String> {
        let id = cert.status.key_secret.as_ref().ok_or_else(|| {
            KeepError::NotFound(format!("certificate {} has no key", cert.meta.id))
        })?;
        let bytes = self.get_secret_payload(id)?;
        String::from_utf8(bytes).map_err(|_| KeepError::Pki("key is not PEM text".into()))
    }

    /// Revoke on the issuing CA and mark the resource.
    pub fn revoke_certificate(&self, id: &Id) -> Result<Certificate> {
        let col = self.store.collection::<Certificate>();
        let cert = col.get(id)?;
        let issuer = cert
            .status
            .issuer
            .clone()
            .unwrap_or_else(|| self.ca.issuing_id().clone());
        self.ca.revoke(&issuer, cert.status.serial)?;
        Ok(col.modify(id, |c| {
            c.status.revoked = true;
            Ok(())
        })?)
    }

    /// Delete a certificate and its key secret.
    pub fn delete_certificate(&self, id: &Id) -> Result<Certificate> {
        let cert = self.store.collection::<Certificate>().delete(id)?;
        if let Some(k) = &cert.status.key_secret {
            match self.store.collection::<StoredSecret>().delete(k) {
                Ok(_) | Err(pistack_store::StoreError::NotFound(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        self.note_usage(cert.meta.project.as_ref(), CERTIFICATES);
        Ok(cert)
    }

    // ---- CAs ---------------------------------------------------------------

    pub fn list_cas(&self) -> Result<Vec<CertificateAuthority>> {
        Ok(self.store.collection::<CertificateAuthority>().list(None)?)
    }

    pub fn get_ca(&self, id: &Id) -> Result<CertificateAuthority> {
        Ok(self.store.collection::<CertificateAuthority>().get(id)?)
    }
}

fn check_payload(payload: &[u8]) -> Result<()> {
    if payload.is_empty() {
        return Err(KeepError::Invalid("payload must not be empty".into()));
    }
    if payload.len() > MAX_PAYLOAD {
        return Err(KeepError::Invalid(format!(
            "payload exceeds {MAX_PAYLOAD} bytes"
        )));
    }
    Ok(())
}

fn check_metadata(secret: &Secret) -> Result<()> {
    check_meta_fields(&secret.meta.labels, &secret.meta.description)?;
    if secret.spec.content_type.len() > MAX_CONTENT_TYPE {
        return Err(KeepError::Invalid(format!(
            "content_type exceeds {MAX_CONTENT_TYPE} bytes"
        )));
    }
    Ok(())
}

/// Label and description limits shared by secrets and certificates.
pub(crate) fn check_meta_fields(
    labels: &BTreeMap<String, String>,
    description: &str,
) -> Result<()> {
    if labels.len() > MAX_LABELS {
        return Err(KeepError::Invalid(format!(
            "at most {MAX_LABELS} labels are allowed"
        )));
    }
    if let Some((k, _)) = labels
        .iter()
        .find(|(k, v)| k.is_empty() || k.len() > MAX_LABEL_KEY || v.len() > MAX_LABEL_VALUE)
    {
        return Err(KeepError::Invalid(format!(
            "label '{}' must have a 1-{MAX_LABEL_KEY} byte key and a value of at most \
             {MAX_LABEL_VALUE} bytes",
            k.chars().take(32).collect::<String>()
        )));
    }
    if description.len() > MAX_DESCRIPTION {
        return Err(KeepError::Invalid(format!(
            "description exceeds {MAX_DESCRIPTION} bytes"
        )));
    }
    Ok(())
}

/// The keep role (`keep.pistack.io/role`) of a secret keep manages itself.
pub(crate) fn managed_role(labels: &BTreeMap<String, String>) -> Option<&str> {
    labels.get(ROLE_LABEL).map(String::as_str)
}

fn reserved_labels(labels: &BTreeMap<String, String>) -> Vec<(&String, &String)> {
    labels
        .iter()
        .filter(|(k, _)| k.starts_with(RESERVED_LABEL_PREFIX))
        .collect()
}

/// Turn a CN into something that fits the resource-name rules.
fn sanitize(cn: &str) -> String {
    let s: String = cn
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.') {
                c
            } else {
                '-'
            }
        })
        .collect();
    s.chars().take(48).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::kind;

    fn client() -> Client {
        Client::open(&Store::in_memory().unwrap()).unwrap()
    }

    #[test]
    fn secret_lifecycle_through_client() {
        let c = client();
        let proj = Id::new(kind::PROJECT);
        let s = c
            .create_secret(
                Secret::new("db", Some(proj.clone()), SecretSpec::default()),
                b"pw1",
            )
            .unwrap();
        assert_eq!(s.status.version, 1);
        assert_eq!(c.get_secret_payload(&s.meta.id).unwrap(), b"pw1");
        let s2 = c
            .rotate_secret(&s.meta.id, b"pw2", Some(s.meta.revision))
            .unwrap();
        assert_eq!(s2.status.version, 2);
        assert!(s2.status.last_rotated.is_some());
        assert_eq!(c.get_secret_payload(&s.meta.id).unwrap(), b"pw2");
        // Stale revision.
        assert!(c.rotate_secret(&s.meta.id, b"pw3", Some(1)).is_err());
        // Metadata update keeps the payload.
        let mut m = c.get_secret(&s.meta.id).unwrap();
        m.meta.description = "prod db".into();
        let m = c.update_secret(m, None).unwrap();
        assert_eq!(m.status.version, 2);
        assert_eq!(c.get_secret_payload(&s.meta.id).unwrap(), b"pw2");
        assert!(c
            .create_secret(Secret::new("e", None, SecretSpec::default()), b"")
            .is_err());
        c.delete_secret(&s.meta.id).unwrap();
        assert!(c.get_secret(&s.meta.id).is_err());
    }

    #[test]
    fn issue_pki_persists_resources_and_keys_decrypt() {
        let c = client();
        let pki = c
            .issue_server_cert("gw.local", &["gw.local".into()])
            .unwrap();
        assert!(pki.ca_pem.matches("BEGIN CERTIFICATE").count() == 2);
        let certs = c.list_certificates(None).unwrap();
        assert_eq!(certs.len(), 1);
        assert_eq!(certs[0].status.cert_pem, pki.cert_pem);
        assert_eq!(c.certificate_key_pem(&certs[0]).unwrap(), pki.key_pem);

        let client_pki = c.issue_client_cert("agent-1").unwrap();
        let der = crate::ca::pem_to_der(&client_pki.cert_pem).unwrap();
        assert_eq!(c.verify_client_cert(&der).unwrap(), "agent-1");
        let cert = c
            .list_certificates(None)
            .unwrap()
            .into_iter()
            .find(|x| x.spec.common_name == "agent-1")
            .unwrap();
        let revoked = c.revoke_certificate(&cert.meta.id).unwrap();
        assert!(revoked.status.revoked);
        assert!(c.verify_client_cert(&der).is_err());
        // Delete removes the key secret too.
        let key = cert.status.key_secret.clone().unwrap();
        c.delete_certificate(&cert.meta.id).unwrap();
        assert!(c.get_secret(&key).is_err());
        assert_eq!(c.list_cas().unwrap().len(), 2);
    }

    #[test]
    fn payload_limit_is_inclusive_on_create_and_rotate() {
        let c = client();
        let at = vec![b'a'; MAX_PAYLOAD];
        let over = vec![b'a'; MAX_PAYLOAD + 1];
        let s = c
            .create_secret(Secret::new("big", None, SecretSpec::default()), &at)
            .unwrap();
        assert_eq!(c.get_secret_payload(&s.meta.id).unwrap().len(), MAX_PAYLOAD);
        assert!(matches!(
            c.create_secret(Secret::new("big2", None, SecretSpec::default()), &over),
            Err(KeepError::Invalid(_))
        ));
        assert!(matches!(
            c.rotate_secret(&s.meta.id, &over, None),
            Err(KeepError::Invalid(_))
        ));
        assert!(matches!(
            c.rotate_secret(&s.meta.id, b"", None),
            Err(KeepError::Invalid(_))
        ));
        c.rotate_secret(&s.meta.id, &at, None).unwrap();
    }

    #[test]
    fn metadata_limits_apply_on_create_and_update() {
        let c = client();
        let mut s = Secret::new("m", None, SecretSpec::default());
        s.meta.description = "d".repeat(MAX_DESCRIPTION);
        for i in 0..MAX_LABELS {
            s.meta
                .labels
                .insert(format!("k{i}"), "v".repeat(MAX_LABEL_VALUE));
        }
        let s = c.create_secret(s, b"x").unwrap();

        let over = |f: fn(&mut Secret)| {
            let mut x = Secret::new("n", None, SecretSpec::default());
            f(&mut x);
            x
        };
        let cases: [fn(&mut Secret); 6] = [
            |x| x.meta.description = "d".repeat(MAX_DESCRIPTION + 1),
            |x| x.spec.content_type = "t".repeat(MAX_CONTENT_TYPE + 1),
            |x| {
                for i in 0..=MAX_LABELS {
                    x.meta.labels.insert(format!("k{i}"), "v".into());
                }
            },
            |x| {
                x.meta
                    .labels
                    .insert("k".repeat(MAX_LABEL_KEY + 1), "v".into());
            },
            |x| {
                x.meta
                    .labels
                    .insert("k".into(), "v".repeat(MAX_LABEL_VALUE + 1));
            },
            |x| {
                x.meta.labels.insert(String::new(), "v".into());
            },
        ];
        for f in cases {
            // Create refuses it...
            assert!(matches!(
                c.create_secret(over(f), b"x"),
                Err(KeepError::Invalid(_))
            ));
            // ...and so does update, which used to skip every check.
            let mut cur = c.get_secret(&s.meta.id).unwrap();
            cur.meta.labels.clear();
            f(&mut cur);
            assert!(matches!(
                c.update_secret(cur, None),
                Err(KeepError::Invalid(_))
            ));
        }
        assert_eq!(
            c.get_secret(&s.meta.id).unwrap().meta.revision,
            s.meta.revision
        );
    }

    #[test]
    fn reserved_role_label_cannot_be_forged_or_stripped() {
        let c = client();
        let proj = Id::new(kind::PROJECT);
        // Callers may not create a secret that claims to be a cert key.
        let mut forged = Secret::new("fake-key", Some(proj.clone()), SecretSpec::default());
        forged
            .meta
            .labels
            .insert(ROLE_LABEL.into(), ROLE_PRIVATE_KEY.into());
        assert!(matches!(
            c.create_secret(forged, b"x"),
            Err(KeepError::Invalid(_))
        ));

        let cert = c
            .issue_certificate(
                Some(proj.clone()),
                "web",
                CertificateSpec {
                    common_name: "web".into(),
                    ..Default::default()
                },
            )
            .unwrap();
        let key_id = cert.status.key_secret.clone().unwrap();
        let key_secret = c.get_secret(&key_id).unwrap();
        // Stripping the label (which would let the key be deleted out from
        // under its certificate) is refused...
        let mut stripped = key_secret.clone();
        stripped.meta.labels.remove(ROLE_LABEL);
        assert!(matches!(
            c.update_secret(stripped, None),
            Err(KeepError::Conflict(_))
        ));
        // ...as is adding it to an ordinary secret...
        let plain = c
            .create_secret(Secret::new("p", Some(proj), SecretSpec::default()), b"x")
            .unwrap();
        let mut tagged = plain.clone();
        tagged
            .meta
            .labels
            .insert(ROLE_LABEL.into(), ROLE_PRIVATE_KEY.into());
        assert!(matches!(
            c.update_secret(tagged, None),
            Err(KeepError::Conflict(_))
        ));
        // ...and rotating the key away from its certificate.
        assert!(matches!(
            c.rotate_secret(&key_id, b"not a key", None),
            Err(KeepError::Conflict(_))
        ));
        assert!(c
            .certificate_key_pem(&cert)
            .unwrap()
            .contains("PRIVATE KEY"));
        // Editing other metadata on the key secret still works.
        let mut desc = key_secret;
        desc.meta.description = "web key".into();
        assert_eq!(
            c.update_secret(desc, None).unwrap().meta.description,
            "web key"
        );
    }

    #[test]
    fn client_round_trips_secrets_through_vault_transit() {
        use crate::backend::{forget_unwrapped, KeyBackend, META_ROOT_KEY_WRAPPED};
        use crate::crypto::META_ROOT_KEY;
        use crate::vault::mock::{MockVault, KEY, TOKEN};

        let vault = MockVault::start();
        let backend = KeyBackend::VaultTransit(vault.config(TOKEN, KEY));
        let store = Store::in_memory().unwrap();
        let c = Client::open_with(&store, &backend).unwrap();
        let s = c
            .create_secret(Secret::new("db", None, SecretSpec::default()), b"pw-vault")
            .unwrap();
        let pki = c
            .issue_server_cert("gw.local", &["gw.local".into()])
            .unwrap();
        assert!(store.get_meta::<String>(META_ROOT_KEY).unwrap().is_none());
        assert!(store
            .get_meta::<serde_json::Value>(META_ROOT_KEY_WRAPPED)
            .unwrap()
            .is_some());

        // Restart: the key comes back from Vault, CA keys and payloads open.
        forget_unwrapped();
        let c2 = Client::open_with(&store, &backend).unwrap();
        assert_eq!(c2.get_secret_payload(&s.meta.id).unwrap(), b"pw-vault");
        assert_eq!(c2.ca_bundle_pem(), pki.ca_pem);
        let cert = c2.list_certificates(None).unwrap().remove(0);
        assert_eq!(c2.certificate_key_pem(&cert).unwrap(), pki.key_pem);

        // The software backend refuses this store rather than starting over.
        assert!(matches!(
            Client::open_with(&store, &KeyBackend::Software),
            Err(KeepError::Backend(_))
        ));
        // A token Vault refuses cannot open it either.
        assert!(matches!(
            Client::open_with(
                &store,
                &KeyBackend::VaultTransit(vault.config("s.revoked", KEY))
            ),
            Err(KeepError::Backend(_))
        ));
    }

    fn limited(secrets: u64, certificates: u64) -> Client {
        client().with_limits(KeepConfig {
            max_secrets_per_project: secrets,
            max_certificates_per_project: certificates,
            ..KeepConfig::default()
        })
    }

    fn denied(r: Result<impl std::fmt::Debug>) -> pistack_types::ApiError {
        match r {
            Err(KeepError::Denied(e)) => e,
            other => panic!("expected a quota refusal, got {other:?}"),
        }
    }

    fn mk(name: &str, project: &Id) -> Secret {
        Secret::new(name, Some(project.clone()), SecretSpec::default())
    }

    fn cert_spec(cn: &str) -> CertificateSpec {
        CertificateSpec {
            common_name: cn.into(),
            ..Default::default()
        }
    }

    #[test]
    fn secrets_per_project_default_limit_is_exact() {
        let c = limited(3, 10);
        let p = Id::new(kind::PROJECT);
        let mut ids = Vec::new();
        for i in 0..3 {
            ids.push(
                c.create_secret(mk(&format!("s{i}"), &p), b"x")
                    .unwrap()
                    .meta
                    .id,
            );
        }
        let e = denied(c.create_secret(mk("s3", &p), b"x"));
        assert_eq!(e.code, pistack_types::error::ErrorCode::QuotaExceeded);
        assert_eq!(e.details["resource"], SECRETS);
        assert_eq!(e.details["limit"], 3);
        assert_eq!(e.details["used"], 3);
        assert_eq!(
            pistack_types::ApiError::from(KeepError::Denied(e))
                .code
                .status(),
            axum::http::StatusCode::FORBIDDEN
        );
        // Other projects and cloud-scoped secrets are not affected.
        c.create_secret(mk("s0", &Id::new(kind::PROJECT)), b"x")
            .unwrap();
        c.create_secret(Secret::new("cloud", None, SecretSpec::default()), b"x")
            .unwrap();
        // A delete frees exactly one slot.
        c.delete_secret(&ids[0]).unwrap();
        c.create_secret(mk("s3", &p), b"x").unwrap();
        assert!(c.create_secret(mk("s4", &p), b"x").is_err());
    }

    #[test]
    fn gate_project_quota_overrides_the_default_and_records_usage() {
        let c = limited(3, 10);
        let store = c.store().clone();
        let mut spec = gate::ProjectSpec::default();
        spec.quotas.insert(SECRETS.into(), 5);
        let p = store
            .collection::<gate::Project>()
            .create(gate::Project {
                meta: pistack_types::Meta::new(kind::PROJECT, "roomy", None),
                spec,
                status: Default::default(),
            })
            .unwrap()
            .meta
            .id;
        let mut last = None;
        for i in 0..5 {
            last = Some(c.create_secret(mk(&format!("s{i}"), &p), b"x").unwrap());
        }
        assert!(denied(c.create_secret(mk("s5", &p), b"x"))
            .message
            .contains("roomy"));
        let usage = |store: &Store| {
            store
                .collection::<gate::Project>()
                .get(&p)
                .unwrap()
                .status
                .usage
                .get(SECRETS)
                .copied()
        };
        assert_eq!(usage(&store), Some(5));
        c.delete_secret(&last.unwrap().meta.id).unwrap();
        assert_eq!(usage(&store), Some(4));

        // A disabled project refuses creates.
        store
            .collection::<gate::Project>()
            .modify(&p, |x| {
                x.spec.enabled = false;
                Ok(())
            })
            .unwrap();
        assert_eq!(
            denied(c.create_secret(mk("off", &p), b"x")).code,
            pistack_types::error::ErrorCode::Forbidden
        );
    }

    #[test]
    fn certificates_per_project_limit_is_exact_and_keys_count_there() {
        // One secret allowed, two certificates: the certificates' key
        // secrets must not use up the secret slot.
        let c = limited(1, 2);
        let p = Id::new(kind::PROJECT);
        c.issue_certificate(Some(p.clone()), "a", cert_spec("a"))
            .unwrap();
        let b = c
            .issue_certificate(Some(p.clone()), "b", cert_spec("b"))
            .unwrap();
        let serial = c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap();
        let e = denied(c.issue_certificate(Some(p.clone()), "c", cert_spec("c")));
        assert_eq!(e.details["resource"], CERTIFICATES);
        assert_eq!(e.details["limit"], 2);
        // Refused before signing: no serial spent, no key left behind.
        assert_eq!(
            c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap(),
            serial
        );
        assert_eq!(c.list_secrets(Some(&p)).unwrap().len(), 2);
        c.create_secret(mk("only", &p), b"x").unwrap();
        assert!(c.create_secret(mk("more", &p), b"x").is_err());
        // Deleting a certificate frees a certificate slot.
        c.delete_certificate(&b.meta.id).unwrap();
        c.issue_certificate(Some(p.clone()), "c", cert_spec("c"))
            .unwrap();
        // Agent/server certificates (no project) are never limited.
        c.issue_client_cert("agent-1").unwrap();
    }

    #[test]
    fn concurrent_creates_cannot_overshoot_the_limit() {
        let c = limited(5, 10);
        let p = Id::new(kind::PROJECT);
        let handles: Vec<_> = (0..16)
            .map(|i| {
                let c = c.clone();
                let p = p.clone();
                std::thread::spawn(move || c.create_secret(mk(&format!("s{i}"), &p), b"x").is_ok())
            })
            .collect();
        let ok = handles
            .into_iter()
            .map(|h| h.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(ok, 5);
        assert_eq!(c.list_secrets(Some(&p)).unwrap().len(), 5);
    }

    #[test]
    fn certificate_metadata_has_the_secret_limits() {
        let c = client();
        let p = Some(Id::new(kind::PROJECT));
        let serial = c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap();
        let too_long = "d".repeat(MAX_DESCRIPTION + 1);
        assert!(matches!(
            c.issue_certificate_with(p.clone(), "x", cert_spec("x"), too_long, BTreeMap::new()),
            Err(KeepError::Invalid(_))
        ));
        let mut many = BTreeMap::new();
        for i in 0..=MAX_LABELS {
            many.insert(format!("k{i}"), "v".to_owned());
        }
        assert!(matches!(
            c.issue_certificate_with(p.clone(), "x", cert_spec("x"), String::new(), many),
            Err(KeepError::Invalid(_))
        ));
        let mut big = BTreeMap::new();
        big.insert("k".to_owned(), "v".repeat(MAX_LABEL_VALUE + 1));
        assert!(matches!(
            c.issue_certificate_with(p.clone(), "x", cert_spec("x"), String::new(), big),
            Err(KeepError::Invalid(_))
        ));
        // Nothing was issued or stored.
        assert_eq!(
            c.store().get_meta::<u64>(crate::ca::META_SERIAL).unwrap(),
            serial
        );
        assert!(c.list_certificates(p.as_ref()).unwrap().is_empty());

        // At the limits it is accepted and stored in one write.
        let mut labels = BTreeMap::new();
        for i in 0..MAX_LABELS {
            labels.insert(format!("k{i}"), "v".repeat(MAX_LABEL_VALUE));
        }
        let cert = c
            .issue_certificate_with(
                p.clone(),
                "x",
                cert_spec("x"),
                "d".repeat(MAX_DESCRIPTION),
                labels,
            )
            .unwrap();
        assert_eq!(cert.meta.labels.len(), MAX_LABELS);
        assert_eq!(cert.meta.revision, 1);
    }
}
