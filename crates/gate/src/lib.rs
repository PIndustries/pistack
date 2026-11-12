// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Identity and access: principals, projects, roles, tokens, quotas.
//!
//! What other modules use from here:
//! * [`Verifier`] / [`verify`] — check a bearer token without touching the store.
//! * [`AuthCaller`] — axum extractor giving the verified [`Caller`].
//! * [`attach_caller`] — gateway middleware that puts the `Caller` in request extensions.
//! * [`authorize`] — the RBAC decision (`policy.rs`).
//! * [`check_quota`] / [`record_usage`] — project quotas (`quota.rs`).
//! * The resource types in [`types`].
//!
//! HTTP lives under the `identity` prefix; see `api.rs`.

pub mod api;
pub mod audit;
pub mod credentials;
pub mod policy;
pub mod quota;
pub mod token;
pub mod types;

use async_trait::async_trait;
use axum::{extract::FromRef, Router};
use pistack_store::Store;
use pistack_types::{
    kind, ApiError, Id, Kind, ListQuery, Meta, Module, ModuleContext, ModuleInfo, Page,
};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

pub use policy::authorize;
pub use quota::{check_quota, check_quota_in, record_usage};
pub use token::{
    attach_caller, is_aws4_authorization, path_under_silo, require_caller, silo_sigv4_skips_bearer,
    verify, AuthCaller, Claims, Keys, Verifier,
};
pub use types::*;

/// Environment variable that fixes the bootstrap admin password.
pub const ADMIN_PASSWORD_ENV: &str = "PISTACK_ADMIN_PASSWORD";
/// Store meta key: id of the admin project.
pub const META_ADMIN_PROJECT: &str = "gate/admin_project";

/// Router state. Clone is cheap.
#[derive(Clone)]
pub struct AppState {
    pub store: Store,
    pub keys: Keys,
}

impl FromRef<AppState> for Verifier {
    fn from_ref(s: &AppState) -> Verifier {
        s.keys.verifier().with_store(&s.store)
    }
}

/// File in the data directory that holds a generated admin password.
pub const ADMIN_PASSWORD_FILE: &str = "admin-password";

#[derive(Default)]
pub struct Gate {
    state: OnceLock<AppState>,
    password_file: Option<PathBuf>,
}

impl Gate {
    /// A gate that writes a generated admin password to `path` (mode 0600)
    /// and keeps it out of the log.
    pub fn with_password_file(path: impl Into<PathBuf>) -> Gate {
        Gate {
            state: OnceLock::new(),
            password_file: Some(path.into()),
        }
    }

    /// Build (once) the shared state from the context's store.
    fn state(&self, ctx: &ModuleContext) -> anyhow::Result<AppState> {
        if let Some(s) = self.state.get() {
            return Ok(s.clone());
        }
        let store = ctx.store::<Store>();
        let keys = Keys::load_or_create(&store)?;
        let st = AppState {
            store: (*store).clone(),
            keys,
        };
        // If another caller raced us, keep theirs; both came from the same store.
        let _ = self.state.set(st.clone());
        Ok(self.state.get().cloned().unwrap_or(st))
    }

    /// The token verifier, once `router` or `start` has run.
    pub fn verifier(&self) -> Option<Verifier> {
        self.state
            .get()
            .map(|s| s.keys.verifier().with_store(&s.store))
    }
}

#[async_trait]
impl Module for Gate {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "gate",
            prefix: "identity",
            summary: "Identity and access: principals, projects, roles, tokens, quotas",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        match self.state(&ctx) {
            Ok(state) => api::router(state),
            Err(e) => {
                tracing::error!(error = %e, "gate: cannot build state; serving 503");
                Router::new().fallback(|| async {
                    ApiError::unavailable("identity service failed to initialise")
                })
            }
        }
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let state = self.state(&ctx)?;
        bootstrap_with(&state.store, self.password_file.as_deref())
    }
}

/// First-run setup: built-in roles, the `admin` project and principal, and a
/// system-scoped admin binding. Idempotent.
pub fn bootstrap(store: &Store) -> anyhow::Result<()> {
    bootstrap_with(store, None)
}

/// [`bootstrap`], writing a generated admin password to `password_file`.
/// With no file the password goes to the log, which is all a caller with no
/// data directory can do.
pub fn bootstrap_with(store: &Store, password_file: Option<&Path>) -> anyhow::Result<()> {
    let roles = store.collection::<Role>();
    for (name, description) in BUILTIN_ROLES {
        if roles.try_get_by_name(name)?.is_none() {
            let mut meta = Meta::new(kind::ROLE, name, None);
            meta.description = description.to_owned();
            roles.create(Role {
                meta,
                spec: RoleSpec { builtin: true },
                status: RoleStatus::default(),
            })?;
            tracing::debug!(role = name, "created built-in role");
        }
    }

    let principals = store.collection::<Principal>();
    if !principals.list(None)?.is_empty() {
        return Ok(());
    }

    let projects = store.collection::<Project>();
    let admin_project = match projects.try_get_by_name(ADMIN_NAME)? {
        Some(p) => p,
        None => {
            let mut meta = Meta::new(kind::PROJECT, ADMIN_NAME, None);
            meta.description = "Cloud administration".to_owned();
            projects.create(Project {
                meta,
                spec: ProjectSpec {
                    parent: None,
                    enabled: true,
                    quotas: Default::default(),
                },
                status: ProjectStatus::default(),
            })?
        }
    };
    store.put_meta(META_ADMIN_PROJECT, &admin_project.meta.id)?;

    let (password, from_env) = match std::env::var(ADMIN_PASSWORD_ENV) {
        Ok(p) if !p.is_empty() => (p, true),
        _ => (credentials::random_password(), false),
    };
    let password_hash = credentials::hash_password(&password)
        .map_err(|e| anyhow::anyhow!("hashing admin password: {e}"))?;
    let admin = principals.create(Principal {
        meta: Meta::new(kind::PRINCIPAL, ADMIN_NAME, None),
        spec: PrincipalSpec {
            kind: PrincipalKind::User,
            email: None,
            password_hash: Some(password_hash),
            enabled: true,
            default_project: Some(admin_project.meta.id.clone()),
        },
        status: PrincipalStatus::default(),
    })?;

    store.collection::<RoleBinding>().create(RoleBinding {
        meta: Meta::new(
            KIND_BINDING,
            binding_name(ADMIN_NAME, ROLE_ADMIN, &Scope::System, None),
            None,
        ),
        spec: RoleBindingSpec {
            principal: admin.meta.id.clone(),
            role: ROLE_ADMIN.to_owned(),
            scope: Scope::System,
        },
        status: RoleBindingStatus::default(),
    })?;

    if from_env {
        tracing::info!(
            principal = ADMIN_NAME,
            "bootstrapped admin with password from {ADMIN_PASSWORD_ENV}"
        );
    } else if let Some(path) = password_file {
        write_password(path, &password)?;
        tracing::warn!(
            principal = ADMIN_NAME,
            file = %path.display(),
            "bootstrapped admin with a generated password; read it from the file, change it, delete the file"
        );
    } else {
        tracing::warn!(
            principal = ADMIN_NAME,
            password = %password,
            "bootstrapped admin with a generated password; this is logged once, change it"
        );
    }
    Ok(())
}

/// Write the password and a newline to a new file only its owner can read.
fn write_password(path: &Path, password: &str) -> anyhow::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .map_err(|e| anyhow::anyhow!("writing {}: {e}", path.display()))?;
    writeln!(file, "{password}")?;
    Ok(())
}

/// Deterministic, readable name for a binding: `alice:member:web` or `admin:admin:system`.
pub(crate) fn binding_name(
    principal: &str,
    role: &str,
    scope: &Scope,
    project_name: Option<&str>,
) -> String {
    match scope {
        Scope::System => format!("{principal}:{role}:system"),
        Scope::Project { project } => {
            format!(
                "{principal}:{role}:{}",
                project_name.unwrap_or(&project.to_string())
            )
        }
    }
}

/// Apply `ListQuery` filters and paging to an in-memory list. Items are
/// ordered by id (UUIDv7, so creation order); `after` is the last id seen.
pub fn paginate<K: Kind>(mut items: Vec<K>, q: &ListQuery) -> Page<K> {
    let labels = q.label_selector();
    items.retain(|k| {
        let m = k.meta();
        q.name.as_deref().is_none_or(|n| m.name == n)
            && labels.iter().all(|(lk, lv)| m.labels.get(lk) == Some(lv))
    });
    items.sort_by_key(|k| k.meta().id.to_string());
    if let Some(after) = q.after.as_deref() {
        items.retain(|k| k.meta().id.to_string().as_str() > after);
    }
    let total = items.len();
    let limit = q.limit.unwrap_or(100).clamp(1, 1000);
    let next = (items.len() > limit).then(|| items[limit - 1].meta().id.to_string());
    items.truncate(limit);
    Page { items, next, total }
}

/// Small extension so bootstrap and handlers can ask "does this name exist"
/// without matching on the error.
pub(crate) trait TryGetByName<K> {
    fn try_get_by_name(&self, name: &str) -> pistack_store::Result<Option<K>>;
}

impl<K: Kind> TryGetByName<K> for pistack_store::Collection<K> {
    fn try_get_by_name(&self, name: &str) -> pistack_store::Result<Option<K>> {
        match self.get_by_name(None, name) {
            Ok(v) => Ok(Some(v)),
            Err(pistack_store::StoreError::NotFound(_)) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// Resolve a project by name or id, mapping the store error to an API error.
pub fn resolve_project(store: &Store, name_or_id: &str) -> Result<Project, ApiError> {
    Ok(store.collection::<Project>().resolve(None, name_or_id)?)
}

/// The admin project's id, if bootstrap has run.
pub fn admin_project(store: &Store) -> Result<Option<Id>, ApiError> {
    Ok(store.get_meta::<Id>(META_ADMIN_PROJECT)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paginate_filters_and_pages() {
        let mut items: Vec<Role> = (0..5)
            .map(|i| {
                let mut m = Meta::new(kind::ROLE, format!("r{i}"), None);
                if i % 2 == 0 {
                    m.labels.insert("even".into(), "yes".into());
                }
                Role {
                    meta: m,
                    spec: RoleSpec::default(),
                    status: RoleStatus::default(),
                }
            })
            .collect();
        items.reverse();
        let page = paginate(
            items.clone(),
            &ListQuery {
                limit: Some(2),
                ..Default::default()
            },
        );
        assert_eq!(page.total, 5);
        assert_eq!(page.items.len(), 2);
        assert_eq!(page.items[0].meta.name, "r0");
        let next = page.next.clone().unwrap();
        let page2 = paginate(
            items.clone(),
            &ListQuery {
                limit: Some(2),
                after: Some(next),
                ..Default::default()
            },
        );
        assert_eq!(page2.items[0].meta.name, "r2");
        let evens = paginate(
            items.clone(),
            &ListQuery {
                labels: Some("even=yes".into()),
                ..Default::default()
            },
        );
        assert_eq!(evens.total, 3);
        assert!(evens.next.is_none());
        let one = paginate(
            items,
            &ListQuery {
                name: Some("r3".into()),
                ..Default::default()
            },
        );
        assert_eq!(one.total, 1);
    }

    #[test]
    fn a_generated_password_goes_to_a_file_only_its_owner_can_read() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(ADMIN_PASSWORD_FILE);
        write_password(&path, "s3cret").unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "s3cret\n");
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn bootstrap_is_idempotent_and_uses_env() {
        let store = Store::in_memory().unwrap();
        bootstrap(&store).unwrap();
        bootstrap(&store).unwrap();
        assert_eq!(store.collection::<Role>().list(None).unwrap().len(), 3);
        assert_eq!(store.collection::<Principal>().list(None).unwrap().len(), 1);
        assert_eq!(
            store.collection::<RoleBinding>().list(None).unwrap().len(),
            1
        );
        let admin = store
            .collection::<Principal>()
            .get_by_name(None, "admin")
            .unwrap();
        assert!(admin.spec.password_hash.is_some());
        assert!(admin_project(&store).unwrap().is_some());
    }
}
