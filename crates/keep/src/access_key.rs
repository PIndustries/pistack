// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Access keys: an id and secret pair for request signing (S3 SigV4).
//!
//! A signature scheme like SigV4 needs the server to recompute an HMAC with
//! the secret, so the secret is sealed like any other payload, not hashed.
//! Each key is a project-scoped [`StoredSecret`] that keep manages itself:
//!
//! * name: the access key id (`PIAK` + 16 base32 characters), stored in the
//!   clear;
//! * labels: `keep.pistack.io/role=access-key`,
//!   `keep.pistack.io/access-key-id=<id>` and `keep.pistack.io/principal=<prin-…>`.
//!   The whole `keep.pistack.io/` prefix is reserved: the secret API
//!   cannot set, change or strip these labels;
//! * payload: the 40-character secret access key, sealed under the root key
//!   with the secret's id as associated data.
//!
//! The secret is returned once, by [`Client::create_access_key`]. The
//! secret routes refuse to read, rotate or delete it; revoking goes through
//! [`Client::revoke_access_key`]. Other modules in the binary verify a
//! request with [`Client::lookup_access_key`], which also returns the
//! project-scoped [`Caller`] to act as. A key works only while its
//! principal and project exist and are enabled and the principal still has
//! a role in the project.
//!
//! Lookup by id is one read of store meta `keep/access-keys/<id>`, which
//! points at the secret. Revoking leaves a tombstone there, so a revoked id
//! is never reissued; lookup refuses it exactly as it refuses an unknown id.
//!
//! Keys count against the project's `access_keys` quota (see `quota.rs`).

use base64::{engine::general_purpose::STANDARD as B64, Engine};
use chrono::{DateTime, Utc};
use pistack_types::{kind, Caller, Id, Meta, Phase};
use rand::{Rng, RngCore};
use serde::{Deserialize, Serialize};

use crate::ca::{open_secret, ROLE_LABEL};
use crate::client::{check_meta_fields, managed_role, Client};
use crate::error::{KeepError, Result};
use crate::quota;
use crate::types::*;

/// [`ROLE_LABEL`] value on an access key's secret.
pub const ROLE_ACCESS_KEY: &str = "access-key";
/// Label holding the access key id.
pub const LABEL_ACCESS_KEY_ID: &str = "keep.pistack.io/access-key-id";
/// Label holding the bound principal's id.
pub const LABEL_PRINCIPAL: &str = "keep.pistack.io/principal";
/// Gate quota resource name for access keys.
pub const ACCESS_KEYS: &str = "access_keys";
/// Every access key id starts with this.
pub const ACCESS_KEY_ID_PREFIX: &str = "PIAK";
/// Length of an access key id, prefix included.
pub const ACCESS_KEY_ID_LEN: usize = 20;
const META_INDEX_PREFIX: &str = "keep/access-keys/";
const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// An access key as the API shows it. Never carries the secret.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct AccessKey {
    /// Id of the secret that holds it (`sec-…`).
    pub id: Id,
    pub access_key_id: String,
    pub project: Id,
    pub principal: Id,
    #[serde(default)]
    pub description: String,
    pub created_at: DateTime<Utc>,
}

/// What create returns: the key plus its secret, shown this once.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreatedAccessKey {
    #[serde(flatten)]
    pub access_key: AccessKey,
    pub secret_access_key: String,
}

impl std::fmt::Debug for CreatedAccessKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CreatedAccessKey")
            .field("access_key", &self.access_key)
            .field("secret_access_key", &"..")
            .finish()
    }
}

/// The one error [`Client::lookup_access_key`] returns for any key that
/// does not resolve to a usable credential.
pub const INVALID_ACCESS_KEY: &str = "invalid access key";

/// What [`Client::lookup_access_key`] hands a verifier.
#[derive(Clone, PartialEq, Eq)]
pub struct AccessKeyCredential {
    pub access_key_id: String,
    pub project: Id,
    pub principal: Id,
    /// The secret access key (ASCII), for recomputing a signature.
    pub secret: Vec<u8>,
    /// Who the request acts as once the signature verifies: the bound
    /// principal scoped to the key's project, with only its roles in that
    /// project. Never `system`, even for a cloud admin, so a leaked key
    /// cannot act outside its project.
    pub caller: Caller,
}

impl std::fmt::Debug for AccessKeyCredential {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AccessKeyCredential")
            .field("access_key_id", &self.access_key_id)
            .field("project", &self.project)
            .field("principal", &self.principal)
            .field("secret", &"..")
            .field("caller", &self.caller)
            .finish()
    }
}

/// Store meta under `keep/access-keys/<id>`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
struct IndexEntry {
    secret: Id,
    project: Id,
    principal: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    revoked_at: Option<DateTime<Utc>>,
}

fn index_key(access_key_id: &str) -> String {
    format!("{META_INDEX_PREFIX}{access_key_id}")
}

/// Shape check only: `PIAK` + 16 of `[A-Z2-7]`.
pub fn is_access_key_id(s: &str) -> bool {
    s.len() == ACCESS_KEY_ID_LEN
        && s.starts_with(ACCESS_KEY_ID_PREFIX)
        && s[ACCESS_KEY_ID_PREFIX.len()..]
            .bytes()
            .all(|b| BASE32.contains(&b))
}

fn new_access_key_id() -> String {
    let mut rng = rand::thread_rng();
    let mut s = String::with_capacity(ACCESS_KEY_ID_LEN);
    s.push_str(ACCESS_KEY_ID_PREFIX);
    for _ in ACCESS_KEY_ID_PREFIX.len()..ACCESS_KEY_ID_LEN {
        s.push(BASE32[rng.gen_range(0..BASE32.len())] as char);
    }
    s
}

/// 30 random bytes, standard base64: 40 characters, like S3's.
fn new_secret() -> String {
    let mut b = [0u8; 30];
    rand::thread_rng().fill_bytes(&mut b);
    B64.encode(b)
}

pub(crate) fn view(s: &Secret) -> Result<AccessKey> {
    let bad = || KeepError::Crypto(format!("access key record {} is malformed", s.meta.id));
    let access_key_id = s.meta.labels.get(LABEL_ACCESS_KEY_ID).ok_or_else(bad)?;
    let principal = s
        .meta
        .labels
        .get(LABEL_PRINCIPAL)
        .and_then(|p| Id::parse(kind::PRINCIPAL, p))
        .ok_or_else(bad)?;
    Ok(AccessKey {
        id: s.meta.id.clone(),
        access_key_id: access_key_id.clone(),
        project: s.meta.project.clone().ok_or_else(bad)?,
        principal,
        description: s.meta.description.clone(),
        created_at: s.meta.created_at,
    })
}

pub(crate) fn is_access_key(s: &Secret) -> bool {
    managed_role(&s.meta.labels) == Some(ROLE_ACCESS_KEY)
}

impl Client {
    /// Mint an access key for `principal` in `project`. Both must be gate
    /// records and enabled. Counts against the project's `access_keys`
    /// quota. The secret is in the result and nowhere else readable.
    pub fn create_access_key(
        &self,
        project: &Id,
        principal: &Id,
        description: &str,
    ) -> Result<CreatedAccessKey> {
        check_meta_fields(&Default::default(), description)?;
        let store = self.store();
        match store.collection::<gate::Principal>().try_get(principal)? {
            Some(p) if p.spec.enabled => {}
            Some(_) => {
                return Err(KeepError::Invalid(format!(
                    "principal {principal} is disabled"
                )))
            }
            None => {
                return Err(KeepError::Invalid(format!(
                    "principal {principal} is not a gate principal"
                )))
            }
        }
        if store
            .collection::<gate::Project>()
            .try_get(project)?
            .is_none()
        {
            return Err(KeepError::Invalid(format!(
                "project {project} is not a gate project"
            )));
        }
        if project_roles(store, principal, project)?.is_empty() {
            return Err(KeepError::Invalid(format!(
                "principal {principal} has no role in project {project}"
            )));
        }
        let secret = new_secret();
        let created = {
            let _admission = quota::admission();
            // Disabled project and quota are both refused here.
            self.admit(Some(project), ACCESS_KEYS)?;
            let mut access_key_id = new_access_key_id();
            let mut tries = 0;
            while store
                .get_meta::<IndexEntry>(&index_key(&access_key_id))?
                .is_some()
            {
                tries += 1;
                if tries > 5 {
                    return Err(KeepError::Conflict(
                        "could not pick an unused access key id".into(),
                    ));
                }
                access_key_id = new_access_key_id();
            }
            let mut meta = Meta::new(kind::SECRET, access_key_id.clone(), Some(project.clone()));
            meta.description = description.to_owned();
            meta.labels
                .insert(ROLE_LABEL.to_owned(), ROLE_ACCESS_KEY.to_owned());
            meta.labels
                .insert(LABEL_ACCESS_KEY_ID.to_owned(), access_key_id.clone());
            meta.labels
                .insert(LABEL_PRINCIPAL.to_owned(), principal.to_string());
            let aad = meta.id.to_string();
            let stored = StoredSecret {
                meta,
                spec: SecretSpec {
                    kind: SecretKind::Token,
                    content_type: "text/plain".into(),
                    expires_at: None,
                    rotate_after_days: None,
                },
                status: StoredSecretStatus {
                    public: SecretStatus {
                        phase: Phase::Ready,
                        version: 1,
                        last_rotated: None,
                    },
                    sealed: Some(self.root_key().seal(secret.as_bytes(), aad.as_bytes())?),
                },
            };
            let col = store.collection::<StoredSecret>();
            let created = col.create(stored)?;
            let entry = IndexEntry {
                secret: created.meta.id.clone(),
                project: project.clone(),
                principal: principal.clone(),
                revoked_at: None,
            };
            if let Err(e) = store.put_meta(&index_key(&access_key_id), &entry) {
                let _ = col.delete(&created.meta.id);
                return Err(e.into());
            }
            created
        };
        self.note_usage(Some(project), ACCESS_KEYS);
        tracing::info!(
            access_key_id = %created.meta.name,
            %project,
            %principal,
            "keep: created access key"
        );
        Ok(CreatedAccessKey {
            access_key: view(&public_secret(created))?,
            secret_access_key: secret,
        })
    }

    /// Access keys, optionally one project's. No secrets.
    pub fn list_access_keys(&self, project: Option<&Id>) -> Result<Vec<AccessKey>> {
        self.store()
            .collection::<Secret>()
            .list(project)?
            .iter()
            .filter(|s| is_access_key(s))
            .map(view)
            .collect()
    }

    /// One access key by the id of its secret (`sec-…`).
    pub fn get_access_key(&self, id: &Id) -> Result<AccessKey> {
        match self.store().collection::<Secret>().try_get(id)? {
            Some(s) if is_access_key(&s) => view(&s),
            _ => Err(KeepError::NotFound(format!("access key {id} not found"))),
        }
    }

    /// One live access key by its access key id (`PIAK…`).
    pub fn find_access_key(&self, access_key_id: &str) -> Result<AccessKey> {
        let not_found = || KeepError::NotFound(format!("access key {access_key_id} not found"));
        if !is_access_key_id(access_key_id) {
            return Err(not_found());
        }
        match self
            .store()
            .get_meta::<IndexEntry>(&index_key(access_key_id))?
        {
            Some(e) if e.revoked_at.is_none() => self.get_access_key(&e.secret),
            _ => Err(not_found()),
        }
    }

    /// Revoke: the id is tombstoned first, then the secret is deleted.
    /// Frees one `access_keys` slot.
    pub fn revoke_access_key(&self, id: &Id) -> Result<AccessKey> {
        let key = self.get_access_key(id)?;
        let store = self.store();
        store.put_meta(
            &index_key(&key.access_key_id),
            &IndexEntry {
                secret: key.id.clone(),
                project: key.project.clone(),
                principal: key.principal.clone(),
                revoked_at: Some(Utc::now()),
            },
        )?;
        match store.collection::<StoredSecret>().delete(id) {
            Ok(_) | Err(pistack_store::StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        self.note_usage(Some(&key.project), ACCESS_KEYS);
        tracing::info!(access_key_id = %key.access_key_id, "keep: revoked access key");
        Ok(key)
    }

    /// For a request verifier in the same binary (e.g. S3 SigV4 in silo):
    /// the secret of a live access key, its project and principal, and the
    /// [`Caller`] to act as once the signature verifies.
    ///
    /// Every key that does not resolve to a usable credential gives the
    /// same error, `KeepError::Unauthorized("invalid access key")`
    /// ([`INVALID_ACCESS_KEY`]): unknown, malformed, revoked, a deleted or
    /// disabled principal or project, or a principal with no role left in
    /// the project. Unknown and revoked ids take the same path (one meta
    /// read, then a decoy decrypt), so neither the error nor the obvious
    /// timing tells them apart. Only a storage failure surfaces as itself.
    pub fn lookup_access_key(&self, access_key_id: &str) -> Result<AccessKeyCredential> {
        match self.resolve_credential(access_key_id)? {
            Some(c) => Ok(c),
            None => {
                self.decoy_decrypt();
                Err(KeepError::Unauthorized(INVALID_ACCESS_KEY.into()))
            }
        }
    }

    /// `Ok(None)` for every "not a usable key" case.
    fn resolve_credential(&self, access_key_id: &str) -> Result<Option<AccessKeyCredential>> {
        let store = self.store();
        // A malformed id still costs one meta read, of a key that never exists.
        let well_formed = is_access_key_id(access_key_id);
        let key = if well_formed {
            index_key(access_key_id)
        } else {
            index_key("-")
        };
        let Some(entry) = store.get_meta::<IndexEntry>(&key)? else {
            return Ok(None);
        };
        if !well_formed || entry.revoked_at.is_some() {
            return Ok(None);
        }
        let Some(secret) = store.collection::<Secret>().try_get(&entry.secret)? else {
            return Ok(None);
        };
        let Ok(key) = view(&secret) else {
            return Ok(None);
        };
        if !is_access_key(&secret)
            || key.access_key_id != access_key_id
            || key.project != entry.project
            || key.principal != entry.principal
        {
            return Ok(None);
        }
        let Some(principal) = store
            .collection::<gate::Principal>()
            .try_get(&key.principal)?
            .filter(|p| p.spec.enabled)
        else {
            return Ok(None);
        };
        if !store
            .collection::<gate::Project>()
            .try_get(&key.project)?
            .is_some_and(|p| p.spec.enabled)
        {
            return Ok(None);
        }
        let roles = project_roles(store, &key.principal, &key.project)?;
        if roles.is_empty() {
            return Ok(None);
        }
        let Ok(bytes) = open_secret(store, self.root_key(), &key.id) else {
            return Ok(None);
        };
        Ok(Some(AccessKeyCredential {
            caller: Caller {
                principal: key.principal.clone(),
                principal_name: principal.meta.name,
                project: Some(key.project.clone()),
                roles,
                system: false,
            },
            access_key_id: key.access_key_id,
            project: key.project,
            principal: key.principal,
            secret: bytes,
        }))
    }

    /// Roughly the cost of opening a real key, so a refusal is not
    /// obviously faster than a success.
    fn decoy_decrypt(&self) {
        let blank = [0u8; 40];
        if let Ok(sealed) = self.root_key().seal(&blank, b"decoy") {
            let _ = self.root_key().open(&sealed, b"decoy");
        }
    }
}

/// The principal's roles bound at `project` scope, sorted. System-scope
/// bindings are deliberately not included.
fn project_roles(
    store: &pistack_store::Store,
    principal: &Id,
    project: &Id,
) -> Result<Vec<String>> {
    let mut roles: Vec<String> = store
        .collection::<gate::RoleBinding>()
        .list(None)?
        .into_iter()
        .filter(|b| &b.spec.principal == principal)
        .filter_map(|b| match b.spec.scope {
            gate::Scope::Project { project: p } if &p == project => Some(b.spec.role),
            _ => None,
        })
        .collect();
    roles.sort();
    roles.dedup();
    Ok(roles)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_store::Store;
    use pistack_types::ApiError;

    struct World {
        c: Client,
        store: Store,
        proj: Id,
        alice: Id,
    }

    fn project(store: &Store, name: &str, quotas: &[(&str, u64)]) -> Id {
        let mut spec = gate::ProjectSpec::default();
        for (k, v) in quotas {
            spec.quotas.insert((*k).into(), *v);
        }
        store
            .collection::<gate::Project>()
            .create(gate::Project {
                meta: Meta::new(kind::PROJECT, name, None),
                spec,
                status: Default::default(),
            })
            .unwrap()
            .meta
            .id
    }

    fn principal(store: &Store, name: &str) -> Id {
        store
            .collection::<gate::Principal>()
            .create(gate::Principal {
                meta: Meta::new(kind::PRINCIPAL, name, None),
                spec: Default::default(),
                status: Default::default(),
            })
            .unwrap()
            .meta
            .id
    }

    fn bind(store: &Store, who: &Id, role: &str, scope: gate::Scope) -> Id {
        let project = match &scope {
            gate::Scope::Project { project } => Some(project.clone()),
            gate::Scope::System => None,
        };
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let name = format!("{role}-{n}");
        store
            .collection::<gate::RoleBinding>()
            .create(gate::RoleBinding {
                meta: Meta::new(gate::KIND_BINDING, name, project),
                spec: gate::RoleBindingSpec {
                    principal: who.clone(),
                    role: role.into(),
                    scope,
                },
                status: Default::default(),
            })
            .unwrap()
            .meta
            .id
    }

    fn world(quotas: &[(&str, u64)]) -> World {
        let store = Store::in_memory().unwrap();
        let c = Client::open(&store).unwrap();
        let proj = project(&store, "p1", quotas);
        let alice = principal(&store, "alice");
        bind(
            &store,
            &alice,
            "member",
            gate::Scope::Project {
                project: proj.clone(),
            },
        );
        World {
            c,
            store,
            proj,
            alice,
        }
    }

    fn invalid() -> String {
        KeepError::Unauthorized(INVALID_ACCESS_KEY.into()).to_string()
    }

    /// Variant and message both, so callers cannot tell cases apart.
    fn assert_invalid(r: Result<AccessKeyCredential>, case: &str) {
        match r {
            Err(e @ KeepError::Unauthorized(_)) => {
                assert_eq!(e.to_string(), invalid(), "{case}");
                let api = ApiError::from(e);
                assert_eq!(api.message, INVALID_ACCESS_KEY, "{case}");
            }
            other => panic!("{case}: expected the invalid-key error, got {other:?}"),
        }
    }

    #[test]
    fn create_list_get_revoke_and_secret_shown_once() {
        let w = world(&[]);
        let k = w.c.create_access_key(&w.proj, &w.alice, "ci").unwrap();
        assert!(is_access_key_id(&k.access_key.access_key_id));
        assert_eq!(k.secret_access_key.len(), 40);
        assert_eq!(k.access_key.project, w.proj);
        assert_eq!(k.access_key.principal, w.alice);
        assert_eq!(k.access_key.description, "ci");
        // Debug never prints the secret.
        assert!(!format!("{k:?}").contains(&k.secret_access_key));

        let list = w.c.list_access_keys(Some(&w.proj)).unwrap();
        assert_eq!(list, vec![k.access_key.clone()]);
        assert!(w
            .c
            .list_access_keys(Some(&Id::new(kind::PROJECT)))
            .unwrap()
            .is_empty());
        assert_eq!(w.c.get_access_key(&k.access_key.id).unwrap(), k.access_key);
        assert_eq!(
            w.c.find_access_key(&k.access_key.access_key_id).unwrap(),
            k.access_key
        );
        // Shown once: the payload route refuses, and the stored row and its
        // public view carry no secret.
        assert!(matches!(
            w.c.get_secret_payload(&k.access_key.id),
            Err(KeepError::Conflict(_))
        ));
        let public = w.c.get_secret(&k.access_key.id).unwrap();
        assert!(!serde_json::to_string(&public)
            .unwrap()
            .contains(&k.secret_access_key));
        let raw = w
            .store
            .collection::<StoredSecret>()
            .get(&k.access_key.id)
            .unwrap();
        assert!(!serde_json::to_string(&raw)
            .unwrap()
            .contains(&k.secret_access_key));

        let revoked = w.c.revoke_access_key(&k.access_key.id).unwrap();
        assert_eq!(revoked, k.access_key);
        assert!(w.c.list_access_keys(Some(&w.proj)).unwrap().is_empty());
        assert!(matches!(
            w.c.get_access_key(&k.access_key.id),
            Err(KeepError::NotFound(_))
        ));
        assert!(matches!(
            w.c.find_access_key(&k.access_key.access_key_id),
            Err(KeepError::NotFound(_))
        ));
        assert!(w.c.get_secret(&k.access_key.id).is_err());
        // Deleting the secret through the client also revokes.
        let k2 = w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
        w.c.delete_secret(&k2.access_key.id).unwrap();
        assert_invalid(
            w.c.lookup_access_key(&k2.access_key.access_key_id),
            "deleted via delete_secret",
        );
    }

    #[test]
    fn create_requires_an_enabled_principal_with_a_role_in_the_project() {
        let w = world(&[]);
        let nobody = Id::new(kind::PRINCIPAL);
        assert!(matches!(
            w.c.create_access_key(&w.proj, &nobody, ""),
            Err(KeepError::Invalid(_))
        ));
        let bob = principal(&w.store, "bob");
        assert!(matches!(
            w.c.create_access_key(&w.proj, &bob, ""),
            Err(KeepError::Invalid(_))
        ));
        // A cloud admin binding is not a role in the project.
        bind(&w.store, &bob, "admin", gate::Scope::System);
        assert!(matches!(
            w.c.create_access_key(&w.proj, &bob, ""),
            Err(KeepError::Invalid(_))
        ));
        assert!(matches!(
            w.c.create_access_key(&Id::new(kind::PROJECT), &w.alice, ""),
            Err(KeepError::Invalid(_))
        ));
        let mut p = w
            .store
            .collection::<gate::Principal>()
            .get(&w.alice)
            .unwrap();
        p.spec.enabled = false;
        w.store
            .collection::<gate::Principal>()
            .update(p, None)
            .unwrap();
        assert!(matches!(
            w.c.create_access_key(&w.proj, &w.alice, ""),
            Err(KeepError::Invalid(_))
        ));
    }

    #[test]
    fn lookup_returns_secret_binding_and_a_project_scoped_caller() {
        let w = world(&[]);
        // Alice is also a cloud admin and has a role in another project;
        // neither leaks into the key's caller.
        bind(&w.store, &w.alice, "admin", gate::Scope::System);
        let other = project(&w.store, "p2", &[]);
        bind(
            &w.store,
            &w.alice,
            "reader",
            gate::Scope::Project { project: other },
        );
        let k = w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
        let cred = w.c.lookup_access_key(&k.access_key.access_key_id).unwrap();
        assert_eq!(cred.access_key_id, k.access_key.access_key_id);
        assert_eq!(cred.project, w.proj);
        assert_eq!(cred.principal, w.alice);
        assert_eq!(cred.secret, k.secret_access_key.as_bytes());
        assert_eq!(
            cred.caller,
            Caller {
                principal: w.alice.clone(),
                principal_name: "alice".into(),
                project: Some(w.proj.clone()),
                roles: vec!["member".into()],
                system: false,
            }
        );
        assert!(!cred.caller.is_admin());
        assert!(!format!("{cred:?}").contains(&k.secret_access_key));
    }

    #[test]
    fn unknown_and_revoked_keys_give_identical_errors() {
        let w = world(&[]);
        let k = w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
        let akid = k.access_key.access_key_id.clone();
        let unknown = w.c.lookup_access_key("PIAKAAAAAAAAAAAAAAAA").unwrap_err();
        w.c.revoke_access_key(&k.access_key.id).unwrap();
        let revoked = w.c.lookup_access_key(&akid).unwrap_err();
        assert!(matches!(unknown, KeepError::Unauthorized(_)));
        assert!(matches!(revoked, KeepError::Unauthorized(_)));
        assert_eq!(unknown.to_string(), revoked.to_string());
        assert_eq!(format!("{unknown:?}"), format!("{revoked:?}"));
        let (a, b) = (ApiError::from(unknown), ApiError::from(revoked));
        assert_eq!(
            serde_json::to_value(&a).unwrap(),
            serde_json::to_value(&b).unwrap()
        );
        assert_invalid(w.c.lookup_access_key(&akid), "revoked");
        for bad in [
            "",
            "PIAK",
            "piakaaaaaaaaaaaaaaaa",
            "PIAK0000000000000000",
            "x/../y",
        ] {
            assert_invalid(w.c.lookup_access_key(bad), bad);
        }
    }

    #[test]
    fn lookup_fails_closed_when_the_binding_goes_away() {
        let w = world(&[]);
        let fresh = |w: &World| {
            w.c.create_access_key(&w.proj, &w.alice, "")
                .unwrap()
                .access_key
                .access_key_id
        };
        // Disabled principal.
        let akid = fresh(&w);
        let pc = w.store.collection::<gate::Principal>();
        let mut p = pc.get(&w.alice).unwrap();
        p.spec.enabled = false;
        let p = pc.update(p, None).unwrap();
        assert_invalid(w.c.lookup_access_key(&akid), "disabled principal");
        let mut p = p;
        p.spec.enabled = true;
        pc.update(p, None).unwrap();
        assert!(w.c.lookup_access_key(&akid).is_ok());

        // Disabled project.
        let prc = w.store.collection::<gate::Project>();
        let mut pr = prc.get(&w.proj).unwrap();
        pr.spec.enabled = false;
        let pr = prc.update(pr, None).unwrap();
        assert_invalid(w.c.lookup_access_key(&akid), "disabled project");
        let mut pr = pr;
        pr.spec.enabled = true;
        prc.update(pr, None).unwrap();

        // No role left in the project.
        let rbc = w.store.collection::<gate::RoleBinding>();
        let binding = rbc
            .list(None)
            .unwrap()
            .into_iter()
            .find(|b| b.spec.principal == w.alice)
            .unwrap();
        rbc.delete(&binding.meta.id).unwrap();
        assert_invalid(w.c.lookup_access_key(&akid), "binding removed");

        // Deleted principal.
        bind(
            &w.store,
            &w.alice,
            "member",
            gate::Scope::Project {
                project: w.proj.clone(),
            },
        );
        assert!(w.c.lookup_access_key(&akid).is_ok());
        pc.delete(&w.alice).unwrap();
        assert_invalid(w.c.lookup_access_key(&akid), "deleted principal");
    }

    #[test]
    fn reserved_labels_are_locked() {
        let w = world(&[]);
        let k = w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
        let id = k.access_key.id.clone();
        // The secret API cannot mint something that looks managed.
        let mut forged = Secret::new("forged", Some(w.proj.clone()), SecretSpec::default());
        forged
            .meta
            .labels
            .insert(ROLE_LABEL.into(), ROLE_ACCESS_KEY.into());
        assert!(matches!(
            w.c.create_secret(forged, b"x"),
            Err(KeepError::Invalid(_))
        ));
        let mut other = Secret::new("other", Some(w.proj.clone()), SecretSpec::default());
        other
            .meta
            .labels
            .insert("keep.pistack.io/anything".into(), "x".into());
        assert!(w.c.create_secret(other, b"x").is_err());

        // Strip, change or add a reserved label: refused.
        let s = w.c.get_secret(&id).unwrap();
        let mut stripped = s.clone();
        stripped.meta.labels.remove(ROLE_LABEL);
        assert!(w.c.update_secret(stripped, None).is_err());
        let mut moved = s.clone();
        moved
            .meta
            .labels
            .insert(LABEL_PRINCIPAL.into(), Id::new(kind::PRINCIPAL).to_string());
        assert!(w.c.update_secret(moved, None).is_err());
        let mut added = s.clone();
        added
            .meta
            .labels
            .insert("keep.pistack.io/extra".into(), "x".into());
        assert!(w.c.update_secret(added, None).is_err());
        // Other labels and the description may change.
        let mut ok = s.clone();
        ok.meta.labels.insert("team".into(), "infra".into());
        ok.meta.description = "renamed".into();
        w.c.update_secret(ok, None).unwrap();
        assert_eq!(w.c.get_access_key(&id).unwrap().description, "renamed");
        // Rotating would replace the secret behind the verifier's back.
        assert!(w.c.rotate_secret(&id, b"new", None).is_err());
        assert_eq!(
            w.c.lookup_access_key(&k.access_key.access_key_id)
                .unwrap()
                .secret,
            k.secret_access_key.as_bytes()
        );
    }

    #[test]
    fn access_key_quota_is_exact_and_separate_from_secrets() {
        let w = world(&[(ACCESS_KEYS, 2), ("secrets", 1)]);
        let a = w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
        w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
        match w.c.create_access_key(&w.proj, &w.alice, "") {
            Err(KeepError::Denied(e)) => {
                let v = serde_json::to_value(&e).unwrap();
                assert_eq!(v["code"], "quota_exceeded", "{v}");
                assert_eq!(v["details"]["resource"], ACCESS_KEYS, "{v}");
                assert_eq!(v["details"]["limit"], 2, "{v}");
            }
            other => panic!("expected quota_exceeded, got {other:?}"),
        }
        // Access keys do not use the secrets budget.
        w.c.create_secret(
            Secret::new("db", Some(w.proj.clone()), SecretSpec::default()),
            b"pw",
        )
        .unwrap();
        let p = w.store.collection::<gate::Project>().get(&w.proj).unwrap();
        assert_eq!(p.status.usage.get(ACCESS_KEYS), Some(&2));
        assert_eq!(p.status.usage.get("secrets"), Some(&1));
        // Revoking frees a slot.
        w.c.revoke_access_key(&a.access_key.id).unwrap();
        w.c.create_access_key(&w.proj, &w.alice, "").unwrap();
    }
}
