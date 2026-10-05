// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `keep::Client`: what the binary and other modules use. Everything the
//! HTTP layer does goes through here too, so the two never disagree.

use chrono::Utc;
use pistack_store::Store;
use pistack_types::{Id, Phase};

use crate::backend::KeyBackend;
use crate::ca::{open_secret, seal_pem_secret, Ca, Issued, ROLE_LABEL, ROLE_PRIVATE_KEY};
use crate::crypto::RootKey;
use crate::error::{KeepError, Result};
use crate::tls::Pki;
use crate::types::*;

/// Largest secret payload accepted (1 MiB).
pub const MAX_PAYLOAD: usize = 1 << 20;
/// Limits on a secret's clear-text metadata, so labels, description and
/// content type cannot carry more than the (sealed) payload is allowed to.
/// Applied the same way on create and update.
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
        })
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
        self.persist_certificate(None, &name, spec, issued)?;
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

    /// Decrypted payload of a secret.
    pub fn get_secret_payload(&self, id: &Id) -> Result<Vec<u8>> {
        open_secret(&self.store, &self.root_key, id)
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
        if secret.meta.labels.contains_key(ROLE_LABEL) {
            return Err(KeepError::Invalid(format!(
                "label {ROLE_LABEL} is reserved for keep"
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
        Ok(public_secret(
            self.store.collection::<StoredSecret>().create(stored)?,
        ))
    }

    /// Replace spec and metadata (not the payload). Honours `If-Match`.
    /// The same metadata limits as create apply, and the reserved role
    /// label cannot be added, changed or removed.
    pub fn update_secret(&self, secret: Secret, expected_revision: Option<u64>) -> Result<Secret> {
        check_metadata(&secret)?;
        let col = self.store.collection::<StoredSecret>();
        let current = col.get(&secret.meta.id)?;
        if secret.meta.labels.get(ROLE_LABEL) != current.meta.labels.get(ROLE_LABEL) {
            return Err(KeepError::Conflict(format!(
                "label {ROLE_LABEL} is managed by keep and cannot be changed"
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
        if is_private_key(&cur.meta.labels) {
            return Err(KeepError::Conflict(
                "this secret holds a certificate's key; it cannot be rotated on its own".into(),
            ));
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

    pub fn delete_secret(&self, id: &Id) -> Result<Secret> {
        Ok(public_secret(
            self.store.collection::<StoredSecret>().delete(id)?,
        ))
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
        let issued = self.ca.issue(&spec)?;
        self.persist_certificate(project, name, spec, issued)
    }

    fn persist_certificate(
        &self,
        project: Option<Id>,
        name: &str,
        spec: CertificateSpec,
        issued: Issued,
    ) -> Result<Certificate> {
        let key_secret = seal_pem_secret(
            &self.store,
            &self.root_key,
            &format!("{name}-key"),
            project.clone(),
            &issued.key_pem,
        )?;
        let mut cert = Certificate::new(name, project, spec);
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
    let m = &secret.meta;
    if m.labels.len() > MAX_LABELS {
        return Err(KeepError::Invalid(format!(
            "at most {MAX_LABELS} labels are allowed"
        )));
    }
    if let Some((k, _)) = m
        .labels
        .iter()
        .find(|(k, v)| k.is_empty() || k.len() > MAX_LABEL_KEY || v.len() > MAX_LABEL_VALUE)
    {
        return Err(KeepError::Invalid(format!(
            "label '{}' must have a 1-{MAX_LABEL_KEY} byte key and a value of at most \
             {MAX_LABEL_VALUE} bytes",
            k.chars().take(32).collect::<String>()
        )));
    }
    if m.description.len() > MAX_DESCRIPTION {
        return Err(KeepError::Invalid(format!(
            "description exceeds {MAX_DESCRIPTION} bytes"
        )));
    }
    if secret.spec.content_type.len() > MAX_CONTENT_TYPE {
        return Err(KeepError::Invalid(format!(
            "content_type exceeds {MAX_CONTENT_TYPE} bytes"
        )));
    }
    Ok(())
}

pub(crate) fn is_private_key(labels: &std::collections::BTreeMap<String, String>) -> bool {
    labels.get(ROLE_LABEL).map(String::as_str) == Some(ROLE_PRIVATE_KEY)
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
}
