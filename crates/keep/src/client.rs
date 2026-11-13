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
    open_secret, pem_to_der, seal_pem_secret, serial_from_der, Ca, Issued, RESERVED_LABEL_PREFIX,
    ROLE_AGENT, ROLE_LABEL, ROLE_PRIVATE_KEY, ROLE_SERVER,
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
        self.issue_pki(cn, sans.to_vec(), CertUsage::Server, None)
    }

    /// Client certificate an agent or service presents for mTLS.
    pub fn issue_client_cert(&self, cn: &str) -> Result<Pki> {
        self.issue_pki(cn, Vec::new(), CertUsage::Client, None)
    }

    /// [`issue_client_cert`](Self::issue_client_cert) for a key the caller
    /// made and shows in `csr_pem`. The returned `key_pem` is empty.
    pub fn issue_client_cert_for(&self, cn: &str, csr_pem: &str) -> Result<Pki> {
        self.issue_pki(cn, Vec::new(), CertUsage::Client, Some(csr_pem))
    }

    /// [`issue_server_cert`](Self::issue_server_cert) for a key the caller
    /// made and shows in `csr_pem`. The returned `key_pem` is empty.
    pub fn issue_server_cert_for(&self, cn: &str, sans: &[String], csr_pem: &str) -> Result<Pki> {
        self.issue_pki(cn, sans.to_vec(), CertUsage::Server, Some(csr_pem))
    }

    fn issue_pki(
        &self,
        cn: &str,
        sans: Vec<String>,
        usage: CertUsage,
        csr_pem: Option<&str>,
    ) -> Result<Pki> {
        let spec = CertificateSpec {
            common_name: cn.to_owned(),
            sans,
            usage,
            ttl_days: default_ttl_days(),
            issuer: None,
        };
        let issued = match csr_pem {
            Some(csr) => self.ca.issue_for_request(&spec, csr)?,
            None => self.ca.issue(&spec)?,
        };
        let name = format!("{}-{}", sanitize(cn), issued.serial);
        let pki = Pki {
            cert_pem: issued.cert_pem.clone(),
            key_pem: issued.key_pem.clone(),
            ca_pem: self.ca.bundle_pem(),
        };
        let role = match usage {
            CertUsage::Client => ROLE_AGENT,
            _ => ROLE_SERVER,
        };
        let labels = BTreeMap::from([(ROLE_LABEL.to_owned(), role.to_owned())]);
        self.persist_certificate(None, &name, spec, issued, String::new(), labels)?;
        Ok(pki)
    }

    /// The cloud's own certificate with this serial: one issued for an
    /// agent or a server listener, not one a project asked for.
    pub fn managed_certificate(&self, serial: u64) -> Result<Option<Certificate>> {
        Ok(self
            .store
            .collection::<Certificate>()
            .list(None)?
            .into_iter()
            .find(|c| c.status.serial == serial && managed_cert_role(c).is_some()))
    }

    /// Renew the certificate with serial `presented`, which the caller has
    /// just proved it holds by using it for TLS. The new one has the same
    /// name, SANs and usage and the key in `csr_pem`; its `key_pem` comes
    /// back empty. The old one is marked as renewed and stays valid until
    /// it expires, so the holder can keep it until the new one works.
    ///
    /// Refused when this cloud has no record of the certificate (it was
    /// deleted, which is how a host is removed for good), when it is
    /// revoked, or when `cn` is not the name it was issued for.
    pub fn renew_certificate(&self, presented: u64, cn: &str, csr_pem: &str) -> Result<Pki> {
        let old = self.managed_certificate(presented)?.ok_or_else(|| {
            KeepError::Unauthorized("this cloud has no record of that certificate".into())
        })?;
        let issuer = old
            .status
            .issuer
            .clone()
            .unwrap_or_else(|| self.ca.issuing_id().clone());
        if old.status.revoked || self.ca.crl(&issuer)?.iter().any(|r| r.serial == presented) {
            return Err(KeepError::Unauthorized("certificate is revoked".into()));
        }
        if old.spec.common_name != cn {
            return Err(KeepError::Denied(pistack_types::ApiError::forbidden(
                format!("certificate is for '{}', not '{cn}'", old.spec.common_name),
            )));
        }
        let pki = self.issue_pki(cn, old.spec.sans.clone(), old.spec.usage, Some(csr_pem))?;
        let serial = serial_from_der(&pem_to_der(&pki.cert_pem)?)?;
        self.store
            .collection::<Certificate>()
            .modify(&old.meta.id, |c| {
                c.status.renewed_by = Some(serial);
                Ok(())
            })?;
        tracing::info!(
            cn,
            old = presented,
            new = serial,
            "keep: certificate renewed"
        );
        Ok(pki)
    }

    /// Issue the successor of a server's listener certificate, given the
    /// certificate itself and a request for a new key.
    ///
    /// When this store has the certificate's record it is renewed like any
    /// other ([`renew_certificate`](Self::renew_certificate)). It may have
    /// none: a follower issues its first listener certificate from its own
    /// copy of the store, which does not replicate, and a restored store
    /// is older than the certificates made since. The certificate still
    /// carries this CA's signature, so one with the same name and SANs is
    /// issued in its place, and from then on there is a record.
    ///
    /// Records are matched by fingerprint, not by serial: a follower's own
    /// copy can have given out a serial this store gave to something else.
    /// A certificate this CA did not sign is refused, which is what keeps
    /// one an operator installed from being replaced.
    pub fn renew_listener_certificate(&self, cert_pem: &str, csr_pem: &str) -> Result<Pki> {
        let current = self.ca.read_listener_cert(&pem_to_der(cert_pem)?)?;
        let record = self
            .store
            .collection::<Certificate>()
            .list(None)?
            .into_iter()
            .find(|c| c.status.fingerprint_sha256 == current.fingerprint_sha256);
        match record {
            Some(rec) if managed_cert_role(&rec) == Some(ROLE_SERVER) => {
                self.renew_certificate(rec.status.serial, &current.common_name, csr_pem)
            }
            Some(_) => Err(KeepError::Denied(pistack_types::ApiError::forbidden(
                "only a server's listener certificate is renewed here",
            ))),
            None => {
                tracing::info!(
                    cn = %current.common_name,
                    "keep: no record of this listener certificate here; issuing one in its place"
                );
                self.issue_server_cert_for(&current.common_name, &current.sans, csr_pem)
            }
        }
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
        // A certificate issued for a request has no key here to keep.
        let key_secret = if issued.key_pem.is_empty() {
            None
        } else {
            Some(seal_pem_secret(
                &self.store,
                &self.root_key,
                &format!("{name}-key"),
                project.clone(),
                &issued.key_pem,
            )?)
        };
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
            key_secret: key_secret.clone(),
            revoked: false,
            renewed_by: None,
        };
        match self.store.collection::<Certificate>().create(cert) {
            Ok(c) => Ok(c),
            Err(e) => {
                // Do not leave an orphaned key behind.
                if let Some(k) = &key_secret {
                    let _ = self.store.collection::<StoredSecret>().delete(k);
                }
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
/// `agent` or `server` for a certificate the cloud issued for itself, and
/// `None` for one a caller asked for. Certificates from before the role
/// label are told by having no project and the name `issue_pki` gives.
pub fn managed_cert_role(c: &Certificate) -> Option<&'static str> {
    if c.meta.project.is_some() {
        return None;
    }
    match c.meta.labels.get(ROLE_LABEL).map(String::as_str) {
        Some(ROLE_AGENT) => return Some(ROLE_AGENT),
        Some(ROLE_SERVER) => return Some(ROLE_SERVER),
        Some(_) => return None,
        None => {}
    }
    let named = format!("{}-{}", sanitize(&c.spec.common_name), c.status.serial);
    if c.meta.name != named {
        return None;
    }
    Some(match c.spec.usage {
        CertUsage::Client => ROLE_AGENT,
        _ => ROLE_SERVER,
    })
}

/// When the agent certificate of `node` expires: the newest one issued for
/// that name that is not revoked. `None` for a host with no certificate.
pub fn agent_cert_expiry(
    certs: &[Certificate],
    node: &str,
) -> Option<chrono::DateTime<chrono::Utc>> {
    certs
        .iter()
        .filter(|c| {
            managed_cert_role(c) == Some(ROLE_AGENT)
                && c.spec.common_name == node
                && !c.status.revoked
        })
        .max_by_key(|c| c.status.serial)
        .and_then(|c| c.status.not_after)
}

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

    fn serial_of(pki: &Pki) -> u64 {
        serial_from_der(&pem_to_der(&pki.cert_pem).unwrap()).unwrap()
    }

    #[test]
    fn a_certificate_is_renewed_for_a_new_key_and_the_old_one_is_marked() {
        let c = client();
        let first = c.issue_client_cert("n1").unwrap();
        let old = serial_of(&first);
        assert_eq!(
            managed_cert_role(&c.managed_certificate(old).unwrap().unwrap()),
            Some(ROLE_AGENT)
        );

        let req = crate::ca::new_key_request("n1").unwrap();
        let renewed = c.renew_certificate(old, "n1", &req.csr_pem).unwrap();
        // The key is the requester's: none comes back and none is stored.
        assert!(renewed.key_pem.is_empty());
        let new = serial_of(&renewed);
        assert_ne!(new, old);
        let rec = c.managed_certificate(new).unwrap().unwrap();
        assert!(rec.status.key_secret.is_none());
        assert_eq!(rec.spec.common_name, "n1");
        assert_eq!(rec.spec.usage, CertUsage::Client);
        assert_eq!(managed_cert_role(&rec), Some(ROLE_AGENT));
        // What the host list shows is the new one's last day.
        let all = c.list_certificates(None).unwrap();
        assert_eq!(agent_cert_expiry(&all, "n1"), rec.status.not_after);
        assert_eq!(agent_cert_expiry(&all, "n9"), None);
        // Both verify: the old one is kept until the new one works.
        assert_eq!(
            c.managed_certificate(old)
                .unwrap()
                .unwrap()
                .status
                .renewed_by,
            Some(new)
        );
        for pki in [&first, &renewed] {
            let der = pem_to_der(&pki.cert_pem).unwrap();
            assert_eq!(c.verify_client_cert(&der).unwrap(), "n1");
        }

        // Another node's name, with this node's certificate.
        let other = crate::ca::new_key_request("n2").unwrap();
        let err = pistack_types::ApiError::from(
            c.renew_certificate(new, "n2", &other.csr_pem).unwrap_err(),
        );
        assert_eq!(err.code, pistack_types::error::ErrorCode::Forbidden);
        // A request for another name than the one asked for.
        assert!(matches!(
            c.renew_certificate(new, "n1", &other.csr_pem),
            Err(KeepError::Invalid(_))
        ));
    }

    #[test]
    fn a_revoked_or_deleted_certificate_is_not_renewed() {
        let c = client();
        let req = || crate::ca::new_key_request("n1").unwrap().csr_pem;
        let count = || c.list_certificates(None).unwrap().len();

        let revoked = serial_of(&c.issue_client_cert("n1").unwrap());
        let id = c.managed_certificate(revoked).unwrap().unwrap().meta.id;
        c.revoke_certificate(&id).unwrap();
        let before = count();
        let err = c.renew_certificate(revoked, "n1", &req()).unwrap_err();
        assert!(
            matches!(&err, KeepError::Unauthorized(m) if m.contains("revoked")),
            "{err}"
        );

        // Revoked on the CA only, the record not marked.
        let on_ca = serial_of(&c.issue_client_cert("n1").unwrap());
        c.ca().revoke(c.ca().issuing_id(), on_ca).unwrap();
        assert!(matches!(
            c.renew_certificate(on_ca, "n1", &req()),
            Err(KeepError::Unauthorized(_))
        ));

        // The record deleted: the host was removed.
        let gone = serial_of(&c.issue_client_cert("n1").unwrap());
        let id = c.managed_certificate(gone).unwrap().unwrap().meta.id;
        c.delete_certificate(&id).unwrap();
        assert!(matches!(
            c.renew_certificate(gone, "n1", &req()),
            Err(KeepError::Unauthorized(_))
        ));
        // A serial this cloud never issued.
        assert!(c.renew_certificate(9999, "n1", &req()).is_err());
        // Nothing was issued by any of the refusals.
        assert_eq!(count(), before + 1);

        // A certificate a project asked for is not the cloud's to renew.
        let project = Id::new(pistack_types::kind::PROJECT);
        let theirs = c
            .issue_certificate(
                Some(project),
                "web",
                CertificateSpec {
                    common_name: "n1".into(),
                    usage: CertUsage::Client,
                    ..Default::default()
                },
            )
            .unwrap();
        assert!(c
            .renew_certificate(theirs.status.serial, "n1", &req())
            .is_err());
    }

    #[test]
    fn a_server_certificate_keeps_its_names_when_renewed() {
        let c = client();
        let sans = vec!["10.0.0.11".to_owned(), "pistack".to_owned()];
        let first = c.issue_server_cert("pistack", &sans).unwrap();
        let req = crate::ca::new_key_request("pistack").unwrap();
        let renewed = c
            .renew_certificate(serial_of(&first), "pistack", &req.csr_pem)
            .unwrap();
        let rec = c.managed_certificate(serial_of(&renewed)).unwrap().unwrap();
        assert_eq!(rec.spec.sans, sans);
        assert_eq!(rec.spec.usage, CertUsage::Server);
        assert_eq!(managed_cert_role(&rec), Some(ROLE_SERVER));
    }

    #[test]
    fn a_listener_certificate_with_no_record_here_is_replaced_by_one_like_it() {
        let c = client();
        let sans = vec!["10.0.0.12".to_owned(), "pistack".to_owned()];
        let req = || crate::ca::new_key_request("pistack").unwrap().csr_pem;
        let count = || c.list_certificates(None).unwrap().len();

        // Recorded: renewed, and the old record says so.
        let known = c.issue_server_cert("pistack", &sans).unwrap();
        let renewed = c
            .renew_listener_certificate(&known.cert_pem, &req())
            .unwrap();
        let old = c.managed_certificate(serial_of(&known)).unwrap().unwrap();
        assert_eq!(old.status.renewed_by, Some(serial_of(&renewed)));

        // As a follower's first certificate is on the leader: signed by
        // this CA, no record. One like it is issued and recorded.
        let local = c.issue_server_cert("pistack", &sans).unwrap();
        let id = c
            .managed_certificate(serial_of(&local))
            .unwrap()
            .unwrap()
            .meta
            .id;
        c.delete_certificate(&id).unwrap();
        let before = count();
        let fresh = c
            .renew_listener_certificate(&local.cert_pem, &req())
            .unwrap();
        assert!(fresh.key_pem.is_empty());
        assert_eq!(count(), before + 1);
        let rec = c.managed_certificate(serial_of(&fresh)).unwrap().unwrap();
        assert_eq!(rec.spec.common_name, "pistack");
        assert_eq!(rec.spec.sans, sans);
        assert_eq!(rec.spec.usage, CertUsage::Server);
        assert_eq!(managed_cert_role(&rec), Some(ROLE_SERVER));

        // The same serial on another record is not the same certificate:
        // that record is left alone and the names come from the one shown.
        let other = c
            .issue_server_cert("pistack", &["10.0.0.99".to_owned()])
            .unwrap();
        let other_id = c
            .managed_certificate(serial_of(&other))
            .unwrap()
            .unwrap()
            .meta
            .id;
        let lost = c.issue_server_cert("pistack", &sans).unwrap();
        let lost_serial = serial_of(&lost);
        let lost_id = c.managed_certificate(lost_serial).unwrap().unwrap().meta.id;
        c.delete_certificate(&lost_id).unwrap();
        c.store()
            .collection::<Certificate>()
            .modify(&other_id, |r| {
                r.status.serial = lost_serial;
                Ok(())
            })
            .unwrap();
        let fresh = c
            .renew_listener_certificate(&lost.cert_pem, &req())
            .unwrap();
        let rec = c.managed_certificate(serial_of(&fresh)).unwrap().unwrap();
        assert_eq!(rec.spec.sans, sans);
        let untouched = c.get_certificate(&other_id).unwrap();
        assert_eq!(untouched.status.renewed_by, None);

        // Not this cloud's CA: an operator's own certificate stays.
        let foreign = client().issue_server_cert("pistack", &sans).unwrap();
        let before = count();
        let err = c
            .renew_listener_certificate(&foreign.cert_pem, &req())
            .unwrap_err();
        assert!(
            matches!(&err, KeepError::Unauthorized(m) if m.contains("not issued")),
            "{err}"
        );
        // An agent's certificate is not a listener's.
        let agent = c.issue_client_cert("n1").unwrap();
        let csr = crate::ca::new_key_request("n1").unwrap().csr_pem;
        assert!(c.renew_listener_certificate(&agent.cert_pem, &csr).is_err());
        assert_eq!(count(), before + 1);
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
