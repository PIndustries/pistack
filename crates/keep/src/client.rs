// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `keep::Client`: what the binary and other modules use. Everything the
//! HTTP layer does goes through here too, so the two never disagree.

use chrono::Utc;
use pistack_store::Store;
use pistack_types::{Id, Phase};

use crate::ca::{open_secret, seal_pem_secret, Ca, Issued};
use crate::crypto::RootKey;
use crate::error::{KeepError, Result};
use crate::tls::Pki;
use crate::types::*;

/// Largest secret payload accepted (1 MiB).
pub const MAX_PAYLOAD: usize = 1 << 20;

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
    /// Open (or on first start, initialise) the software key backend and
    /// the cloud CA on `store`.
    pub fn open(store: &Store) -> Result<Self> {
        let root_key = RootKey::load_or_create(store)?;
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
    pub fn update_secret(&self, secret: Secret, expected_revision: Option<u64>) -> Result<Secret> {
        let col = self.store.collection::<StoredSecret>();
        let current = col.get(&secret.meta.id)?;
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
}
