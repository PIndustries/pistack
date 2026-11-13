// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resources `keep` owns: Secret, Certificate, CertificateAuthority and
//! JoinToken.
//!
//! Each is a `Resource<Spec, Status>` keyed by the `Spec` type's `KIND`.
//! Secret payloads and private keys are never fields of the wire types: they
//! are sealed with the root key (`crypto.rs`) and live in a store-only
//! status wrapper ([`StoredSecret`]) that the API strips before responding.

use chrono::{DateTime, Utc};
use pistack_types::{kind, Id, Phase, Resource, Spec};
use serde::{Deserialize, Serialize};

use crate::crypto::Sealed;

/// Id kind for certificate authorities (keep-private).
pub const KIND_CA: &str = "ca";
/// Id kind for join tokens (keep-private).
pub const KIND_JOIN_TOKEN: &str = "jtok";

// ---- Secret ------------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum SecretKind {
    #[default]
    Opaque,
    Password,
    SshKey,
    Token,
    /// A certificate or private key in PEM form.
    Certificate,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct SecretSpec {
    #[serde(default)]
    pub kind: SecretKind,
    /// MIME type of the payload, e.g. `text/plain`, `application/x-pem-file`.
    #[serde(default = "default_content_type")]
    pub content_type: String,
    /// When the payload stops being valid. Informational; nothing deletes it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// How often the owner means to replace the payload. Nothing rotates
    /// it: once the payload is older than this the leader logs a warning
    /// (`sweep.rs`). `None` or `0` = never.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rotate_after_days: Option<u32>,
}

impl Default for SecretSpec {
    fn default() -> Self {
        Self {
            kind: SecretKind::Opaque,
            content_type: default_content_type(),
            expires_at: None,
            rotate_after_days: None,
        }
    }
}

fn default_content_type() -> String {
    "application/octet-stream".to_owned()
}

impl Spec for SecretSpec {
    const KIND: &'static str = kind::SECRET;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct SecretStatus {
    #[serde(default)]
    pub phase: Phase,
    /// Payload version; every rotation bumps it. Starts at 1.
    #[serde(default)]
    pub version: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_rotated: Option<DateTime<Utc>>,
}

/// Wire shape of a secret: metadata only, never the payload.
pub type Secret = Resource<SecretSpec, SecretStatus>;

/// Store-only status: the public status plus the sealed payload.
///
/// `Secret` deserialises from the same JSON (serde ignores `sealed`), which
/// is what keeps the payload out of every response for free. The flip side:
/// writing a `Secret` back through the store would drop the payload, so every
/// write in this crate goes through `StoredSecret`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StoredSecretStatus {
    #[serde(flatten)]
    pub public: SecretStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sealed: Option<Sealed>,
}

pub type StoredSecret = Resource<SecretSpec, StoredSecretStatus>;

/// Strip the sealed payload for the wire.
pub fn public_secret(s: StoredSecret) -> Secret {
    Secret {
        meta: s.meta,
        spec: s.spec,
        status: s.status.public,
    }
}

// ---- Certificate -------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CertUsage {
    #[default]
    Server,
    Client,
    Both,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CertificateSpec {
    pub common_name: String,
    /// DNS names and IP addresses. The common name is not added automatically.
    #[serde(default)]
    pub sans: Vec<String>,
    #[serde(default)]
    pub usage: CertUsage,
    #[serde(default = "default_ttl_days")]
    pub ttl_days: u32,
    /// CA to sign with. `None` = the issuing CA.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<Id>,
}

pub fn default_ttl_days() -> u32 {
    365
}

impl Default for CertificateSpec {
    fn default() -> Self {
        Self {
            common_name: String::new(),
            sans: Vec::new(),
            usage: CertUsage::Server,
            ttl_days: default_ttl_days(),
            issuer: None,
        }
    }
}

impl Spec for CertificateSpec {
    const KIND: &'static str = kind::CERT;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CertificateStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub serial: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<DateTime<Utc>>,
    /// Hex SHA-256 of the DER certificate.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint_sha256: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cert_pem: String,
    /// CA that signed it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issuer: Option<Id>,
    /// Secret holding the encrypted private key (PEM).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_secret: Option<Id>,
    #[serde(default)]
    pub revoked: bool,
    /// Serial of the certificate issued to replace this one at renewal.
    /// This one stays valid until it expires.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub renewed_by: Option<u64>,
}

pub type Certificate = Resource<CertificateSpec, CertificateStatus>;

// ---- CertificateAuthority ----------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CaSpec {
    pub common_name: String,
    pub ttl_days: u32,
    /// `None` for the self-signed root.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Id>,
}

impl Spec for CaSpec {
    const KIND: &'static str = KIND_CA;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Revocation {
    pub serial: u64,
    pub revoked_at: DateTime<Utc>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct CaStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cert_pem: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub fingerprint_sha256: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_before: Option<DateTime<Utc>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub not_after: Option<DateTime<Utc>>,
    /// Last serial this CA issued (the counter itself lives in store meta).
    #[serde(default)]
    pub serial: u64,
    /// Secret holding the encrypted CA private key.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub key_secret: Option<Id>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub revoked: Vec<Revocation>,
}

/// Cloud-scoped (`meta.project == None`), admin only.
pub type CertificateAuthority = Resource<CaSpec, CaStatus>;

// ---- JoinToken ---------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JoinTokenSpec {
    /// Hex SHA-256 of the token. The token itself is shown once at creation.
    pub hash: String,
    pub expires_at: DateTime<Utc>,
    /// `false` = burned on first redeem.
    #[serde(default)]
    pub multi_use: bool,
    /// Most redemptions a multi-use token allows. Set on every token minted
    /// since the cap exists; `None` only on older records, which expire
    /// within the 24h token lifetime. Ignored for single-use tokens.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_uses: Option<u64>,
    /// Principal that created it (for the audit trail).
    pub created_by: String,
}

impl Spec for JoinTokenSpec {
    const KIND: &'static str = KIND_JOIN_TOKEN;
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct JoinTokenStatus {
    #[serde(default)]
    pub uses: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_used: Option<DateTime<Utc>>,
}

/// Cloud-scoped, admin only.
pub type JoinToken = Resource<JoinTokenSpec, JoinTokenStatus>;

pub fn join_token_expired(t: &JoinToken, now: DateTime<Utc>) -> bool {
    t.spec.expires_at <= now
}

/// How many redemptions `t` allows: 1 for a single-use token, `max_uses`
/// for a multi-use one (`None` only on records older than the cap).
pub fn join_token_max_uses(t: &JoinToken) -> Option<u64> {
    if t.spec.multi_use {
        t.spec.max_uses
    } else {
        Some(1)
    }
}

/// No redemptions left: a single-use token that was redeemed, or a
/// multi-use token that reached `max_uses`.
pub fn join_token_spent(t: &JoinToken) -> bool {
    join_token_max_uses(t).is_some_and(|max| t.status.uses >= max)
}

/// What `pistack agent --join` gets for a valid token: enough to request
/// its certificates and register with `forge`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct JoinGrant {
    /// The token that was redeemed.
    pub token: Id,
    /// Name of the token resource; agents may use it as a cohort label.
    pub token_name: String,
    pub created_by: String,
    pub expires_at: DateTime<Utc>,
    pub remaining_uses: Option<u64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_store::{Store, StoreError};
    use pistack_types::Kind;

    /// CONTRIBUTING: every kind creates, lists, updates with a stale revision
    /// (expects conflict) and deletes.
    fn roundtrip<K: Kind>(store: &Store, mut make: impl FnMut(&str) -> K, bump: impl Fn(&mut K)) {
        let col = store.collection::<K>();
        let a = col.create(make("a")).unwrap();
        col.create(make("b")).unwrap();
        assert_eq!(col.list(None).unwrap().len(), 2);
        let mut stale = a.clone();
        let fresh = col
            .modify(&a.meta().id, |x| {
                bump(x);
                Ok(())
            })
            .unwrap();
        assert_eq!(fresh.meta().revision, 2);
        bump(&mut stale);
        assert!(matches!(
            col.update(stale, Some(1)),
            Err(StoreError::Revision { .. })
        ));
        col.delete(&a.meta().id).unwrap();
        assert_eq!(col.list(None).unwrap().len(), 1);
    }

    #[test]
    fn every_kind_roundtrips() {
        let store = Store::in_memory().unwrap();
        let proj = Id::new(kind::PROJECT);
        roundtrip(
            &store,
            |n| StoredSecret::new(n, Some(proj.clone()), SecretSpec::default()),
            |s| s.status.public.version += 1,
        );
        roundtrip(
            &store,
            |n| Certificate::new(n, Some(proj.clone()), CertificateSpec::default()),
            |c| c.status.revoked = true,
        );
        roundtrip(
            &store,
            |n| {
                CertificateAuthority::new(
                    n,
                    None,
                    CaSpec {
                        common_name: n.into(),
                        ttl_days: 1,
                        parent: None,
                    },
                )
            },
            |c| c.status.serial += 1,
        );
        roundtrip(
            &store,
            |n| {
                JoinToken::new(
                    n,
                    None,
                    JoinTokenSpec {
                        hash: "h".into(),
                        expires_at: Utc::now(),
                        multi_use: false,
                        max_uses: None,
                        created_by: "admin".into(),
                    },
                )
            },
            |t| t.status.uses += 1,
        );
    }

    #[test]
    fn secret_wire_type_drops_sealed_payload() {
        let store = Store::in_memory().unwrap();
        let mut s = StoredSecret::new("s", None, SecretSpec::default());
        s.status.sealed = Some(Sealed {
            alg: "aes-256-gcm".into(),
            nonce: "n".into(),
            ciphertext: "c".into(),
        });
        s.status.public.version = 1;
        let s = store.collection::<StoredSecret>().create(s).unwrap();
        let wire = store.collection::<Secret>().get(&s.meta.id).unwrap();
        assert_eq!(wire.status.version, 1);
        let json = serde_json::to_string(&wire).unwrap();
        assert!(!json.contains("ciphertext"));
        assert!(!json.contains("sealed"));
        // Stored form keeps it.
        let back = store.collection::<StoredSecret>().get(&s.meta.id).unwrap();
        assert!(back.status.sealed.is_some());
        let json = serde_json::to_string(&back).unwrap();
        assert!(json.contains("\"version\":1"));
    }
}
