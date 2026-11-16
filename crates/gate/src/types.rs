// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The resources `gate` owns: Project, Principal, Role, RoleBinding, ApiKey.
//!
//! Every one is a `Resource<Spec, Status>` keyed by a `Kind` so it lives in
//! the shared store like everything else. Names and descriptions live in
//! `Meta`; the spec carries only what is particular to the kind.

use chrono::{DateTime, Utc};
use pistack_types::{kind, Id, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Id kind for role bindings (not in the shared list; gate-private).
pub const KIND_BINDING: &str = "rb";
/// Id kind for API keys.
pub const KIND_KEY: &str = "key";

/// Built-in role names. `authorize` in `policy.rs` knows these three.
pub const ROLE_ADMIN: &str = "admin";
pub const ROLE_MEMBER: &str = "member";
pub const ROLE_READER: &str = "reader";
pub const BUILTIN_ROLES: [(&str, &str); 3] = [
    (ROLE_ADMIN, "Full control over the cloud or a project"),
    (
        ROLE_MEMBER,
        "Create, change and delete resources within a project",
    ),
    (ROLE_READER, "Read-only access within a project"),
];

/// Name of the project and principal created on first boot.
pub const ADMIN_NAME: &str = "admin";

/// Define a resource struct with the `Resource<Spec, Status>` wire shape and
/// implement `Kind` for it.
///
/// `Kind` and `Resource` both live in `pistack-types`, so a module cannot
/// `impl Kind for Resource<MySpec, MyStatus>` (orphan rule). Each kind is
/// therefore its own struct with the same fields and serde layout; `From`
/// conversions to and from the generic envelope keep it interchangeable.
macro_rules! resource {
    ($(#[$doc:meta])* $name:ident, $spec:ty, $status:ty, $k:expr) => {
        $(#[$doc])*
        #[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
        pub struct $name {
            #[serde(flatten)]
            pub meta: Meta,
            pub spec: $spec,
            pub status: $status,
        }
        impl Kind for $name {
            const KIND: &'static str = $k;
            fn meta(&self) -> &Meta {
                &self.meta
            }
            fn meta_mut(&mut self) -> &mut Meta {
                &mut self.meta
            }
        }
        impl From<Resource<$spec, $status>> for $name {
            fn from(r: Resource<$spec, $status>) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
        impl From<$name> for Resource<$spec, $status> {
            fn from(r: $name) -> Self {
                Self { meta: r.meta, spec: r.spec, status: r.status }
            }
        }
    };
}

// ---- Project ---------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ProjectSpec {
    /// Parent project for hierarchical quotas; `None` for top-level projects.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<Id>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Resource name -> hard limit. Absent means unlimited.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub quotas: BTreeMap<String, u64>,
}

impl Default for ProjectSpec {
    fn default() -> Self {
        Self {
            parent: None,
            enabled: true,
            quotas: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ProjectStatus {
    /// Resource name -> usage, as last counted by the module that owns the
    /// resource (`quota::mirror_usage`). Kept for reading; a check counts again.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub usage: BTreeMap<String, u64>,
}

resource!(
    /// A tenant. Quotas live in `spec`, usage in `status`.
    Project,
    ProjectSpec,
    ProjectStatus,
    kind::PROJECT
);

// ---- Principal -------------------------------------------------------------

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum PrincipalKind {
    #[default]
    User,
    /// What pods, agents and automation authenticate as.
    ServiceAccount,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PrincipalSpec {
    #[serde(default)]
    pub kind: PrincipalKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub email: Option<String>,
    /// Argon2id PHC string. Never returned by the API; handlers redact it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password_hash: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// Project a token is scoped to when the login does not name one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_project: Option<Id>,
}

impl Default for PrincipalSpec {
    fn default() -> Self {
        Self {
            kind: PrincipalKind::User,
            email: None,
            password_hash: None,
            enabled: true,
            default_project: None,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PrincipalStatus {
    /// Tokens issued at or before this moment are refused. Set by
    /// `POST /principals/{id}/revoke-tokens`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_not_before: Option<DateTime<Utc>>,
}

resource!(
    /// A user or service account. Names are global (no project).
    Principal,
    PrincipalSpec,
    PrincipalStatus,
    kind::PRINCIPAL
);

impl Principal {
    /// Copy with the password hash removed, for API responses.
    pub fn redacted(&self) -> Principal {
        let mut p = self.clone();
        p.spec.password_hash = None;
        p
    }
}

// ---- Role ------------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RoleSpec {
    /// Built-in roles cannot be deleted or renamed.
    #[serde(default)]
    pub builtin: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RoleStatus {}

resource!(
    /// A role. Its `name` and `description` are in `meta`.
    Role,
    RoleSpec,
    RoleStatus,
    kind::ROLE
);

// ---- RoleBinding -----------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Scope {
    /// Cloud-wide.
    System,
    Project {
        project: Id,
    },
}

impl Scope {
    pub fn project(&self) -> Option<&Id> {
        match self {
            Scope::System => None,
            Scope::Project { project } => Some(project),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoleBindingSpec {
    pub principal: Id,
    /// Role name (roles are global, so the name is the key).
    pub role: String,
    pub scope: Scope,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct RoleBindingStatus {}

resource!(
    /// Grants `role` to `principal` at `scope`. `meta.project` mirrors a project scope.
    RoleBinding,
    RoleBindingSpec,
    RoleBindingStatus,
    KIND_BINDING
);

// ---- ApiKey ----------------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiKeySpec {
    pub principal: Id,
    /// Hex SHA-256 of the secret. The secret itself is shown once at creation.
    pub hash: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expires_at: Option<DateTime<Utc>>,
    /// Project the resulting tokens are scoped to. `None` = the principal's
    /// default project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<Id>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ApiKeyStatus {}

resource!(
    /// A long-lived credential. Named `<principal>/<name>`; `meta.project` is its scope.
    ApiKey,
    ApiKeySpec,
    ApiKeyStatus,
    KIND_KEY
);

impl ApiKey {
    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        self.spec.expires_at.is_some_and(|t| t <= now)
    }
}

fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_store::{Store, StoreError};

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
        let prin = Id::new(kind::PRINCIPAL);
        roundtrip(
            &store,
            |n| Project {
                meta: Meta::new(kind::PROJECT, n, None),
                spec: ProjectSpec::default(),
                status: ProjectStatus::default(),
            },
            |p| p.spec.enabled = false,
        );
        roundtrip(
            &store,
            |n| Principal {
                meta: Meta::new(kind::PRINCIPAL, n, None),
                spec: PrincipalSpec::default(),
                status: PrincipalStatus::default(),
            },
            |p| p.spec.enabled = false,
        );
        roundtrip(
            &store,
            |n| Role {
                meta: Meta::new(kind::ROLE, n, None),
                spec: RoleSpec::default(),
                status: RoleStatus::default(),
            },
            |r| r.meta.description = "x".into(),
        );
        roundtrip(
            &store,
            |n| RoleBinding {
                meta: Meta::new(KIND_BINDING, n, Some(proj.clone())),
                spec: RoleBindingSpec {
                    principal: prin.clone(),
                    role: "member".into(),
                    scope: Scope::Project {
                        project: proj.clone(),
                    },
                },
                status: RoleBindingStatus::default(),
            },
            |b| b.spec.role = "reader".into(),
        );
        roundtrip(
            &store,
            |n| ApiKey {
                meta: Meta::new(KIND_KEY, n, Some(proj.clone())),
                spec: ApiKeySpec {
                    principal: prin.clone(),
                    hash: "h".into(),
                    expires_at: None,
                    project: Some(proj.clone()),
                },
                status: ApiKeyStatus::default(),
            },
            |k| k.spec.expires_at = Some(Utc::now()),
        );
    }

    #[test]
    fn scope_serialises_tagged() {
        let s = serde_json::to_value(Scope::System).unwrap();
        assert_eq!(s, serde_json::json!({"type": "system"}));
        let p = Id::new(kind::PROJECT);
        let s = serde_json::to_value(Scope::Project { project: p.clone() }).unwrap();
        assert_eq!(s["type"], "project");
        assert_eq!(s["project"], p.to_string());
    }
}
