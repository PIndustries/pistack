// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Count limits on what `keep` stores.
//!
//! Secrets, certificates and access keys are project resources and use
//! gate's project quotas: the limit for `secrets`, `certificates` or
//! `access_keys` is the project's `spec.quotas` entry (set with
//! `pi project` / `PUT /identity/projects`) and, when the project has none,
//! the module default from [`KeepConfig`]. Usage is counted from the stored
//! rows at admission, so it cannot drift, and is mirrored into the
//! project's `status.usage` for display. A disabled project refuses
//! creates. A certificate's key secret counts against `certificates` and an
//! access key against `access_keys`, not `secrets`. Cloud-scoped rows (no
//! project) are not limited, as elsewhere.
//!
//! Join tokens are cloud-scoped, so their limits are cloud-wide: at most
//! [`KeepConfig::max_live_join_tokens`] unexpired, unspent tokens at once,
//! and a multi-use token may be redeemed at most
//! [`KeepConfig::max_join_token_uses`] times.
//!
//! ## How exact the limits are
//!
//! Count, check and create run under one process-wide lock
//! ([`admission`]), so two concurrent creates in a process cannot both take
//! the last slot. A join-token redeem re-checks `max_uses` inside a
//! single-key compare-and-set (`Collection::modify`, retried on a revision
//! mismatch).
//!
//! Across control-plane nodes the store has one writer. Followers answer
//! every mutating request, `/rpc` included, with `409 not the leader`
//! (`reject_follower_writes` in the binary) and only apply the leader's
//! log. So every create runs in the leader's process, under this lock, and
//! in steady state every limit is exact cluster-wide.
//!
//! The store has no transaction or compare-and-set that spans a count and
//! an insert: `modify` covers one key, and a batch is atomic but cannot
//! read. Nothing stronger is built here, because it would not help: the
//! only way to over-shoot is two nodes accepting writes at once. That
//! happens when a leader misses the election that replaced it. It keeps
//! accepting writes until a vote request with a higher term reaches it.
//! Each acting leader then admits up to the limit against its own copy, so
//! each copy stays within the limit while they are apart. A node that then
//! steps down applies the new leader's log after its own commit index, so
//! its copy can hold its own rows from the split plus the new leader's.
//! The bound per project (per cloud for join tokens) is therefore
//! `limit × k`, where `k` is the number of nodes that accepted writes
//! during the split: twice the limit with two. Redemptions of one join
//! token have the same bound, `max_uses × k`. The new leader's copy, which
//! the cluster follows from then on, never exceeds the limit.

use parking_lot::{Mutex, MutexGuard};
use pistack_store::Store;
use pistack_types::{kind, ApiError, Id, Meta};
use serde::{Deserialize, Serialize};
use std::sync::OnceLock;

use crate::error::{KeepError, Result};

/// Gate quota resource name for secrets.
pub const SECRETS: &str = "secrets";
/// Gate quota resource name for certificates.
pub const CERTIFICATES: &str = "certificates";

pub const DEFAULT_MAX_SECRETS_PER_PROJECT: u64 = 1000;
pub const DEFAULT_MAX_CERTIFICATES_PER_PROJECT: u64 = 500;
pub const DEFAULT_MAX_ACCESS_KEYS_PER_PROJECT: u64 = 100;
pub const DEFAULT_MAX_LIVE_JOIN_TOKENS: u64 = 100;
pub const DEFAULT_MAX_JOIN_TOKEN_USES: u64 = 100;
/// Seven days.
pub const DEFAULT_JOIN_TOKEN_GRACE_SECS: u64 = 7 * 24 * 60 * 60;

/// Module settings under `[modules.keep]` / `PISTACK__MODULES__KEEP__*`.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct KeepConfig {
    /// `secrets` limit for a project whose quotas do not name one.
    pub max_secrets_per_project: u64,
    /// `certificates` limit for a project whose quotas do not name one.
    pub max_certificates_per_project: u64,
    /// `access_keys` limit for a project whose quotas do not name one.
    pub max_access_keys_per_project: u64,
    /// Unexpired, unspent join tokens allowed at once, cloud-wide.
    pub max_live_join_tokens: u64,
    /// Most redemptions a multi-use join token may allow (and its default).
    pub max_join_token_uses: u64,
    /// How long an expired or used-up join token is kept before the
    /// leader's sweep deletes it (see `sweep.rs`). `0` deletes it on the
    /// next sweep.
    pub join_token_grace_secs: u64,
}

impl Default for KeepConfig {
    fn default() -> Self {
        Self {
            max_secrets_per_project: DEFAULT_MAX_SECRETS_PER_PROJECT,
            max_certificates_per_project: DEFAULT_MAX_CERTIFICATES_PER_PROJECT,
            max_access_keys_per_project: DEFAULT_MAX_ACCESS_KEYS_PER_PROJECT,
            max_live_join_tokens: DEFAULT_MAX_LIVE_JOIN_TOKENS,
            max_join_token_uses: DEFAULT_MAX_JOIN_TOKEN_USES,
            join_token_grace_secs: DEFAULT_JOIN_TOKEN_GRACE_SECS,
        }
    }
}

impl KeepConfig {
    /// [`join_token_grace_secs`](Self::join_token_grace_secs) as a duration.
    pub fn join_token_grace(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.join_token_grace_secs)
    }

    /// Read `[modules.keep]` from a loaded config. Missing section → defaults.
    pub fn from_config(cfg: &pistack_config::Config) -> std::result::Result<Self, String> {
        let c: Self = cfg.module("keep").map_err(|e| e.to_string())?;
        if c.max_join_token_uses == 0 {
            return Err("modules.keep.max_join_token_uses must be at least 1".into());
        }
        Ok(c)
    }

    /// Load process config (file + `PISTACK__*`) and take `[modules.keep]`.
    /// A config that does not load or parse logs a warning and yields the
    /// defaults, which are limits too, so nothing becomes unlimited.
    pub fn load() -> Self {
        match pistack_config::Config::load(None)
            .map_err(|e| e.to_string())
            .and_then(|cfg| Self::from_config(&cfg))
        {
            Ok(c) => c,
            Err(e) => {
                tracing::warn!(error = %e, "keep: using default limits");
                Self::default()
            }
        }
    }

    /// The process's settings, loaded once.
    pub fn configured() -> &'static KeepConfig {
        static CONFIGURED: OnceLock<KeepConfig> = OnceLock::new();
        CONFIGURED.get_or_init(Self::load)
    }
}

/// Held across count → check → create. Process-wide; see the module docs
/// for why that is cluster-wide in steady state and what a split can do.
pub(crate) fn admission() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    LOCK.lock()
}

/// Refuse one more `resource` in `project` when `used` already reaches its
/// limit: the project's gate quota, else `default_limit`. Uses gate's
/// check, so the error (403 `quota_exceeded` with limit/used details) and
/// the disabled-project rule match every other module.
pub(crate) fn admit(
    store: &Store,
    project: &Id,
    resource: &str,
    used: u64,
    default_limit: u64,
) -> Result<()> {
    let mut p = match store.collection::<gate::Project>().try_get(project)? {
        Some(p) => p,
        None => {
            // Not a gate project (tests, or ids from elsewhere): the
            // module default still applies.
            let mut meta = Meta::new(kind::PROJECT, project.to_string(), None);
            meta.id = project.clone();
            gate::Project {
                meta,
                spec: Default::default(),
                status: Default::default(),
            }
        }
    };
    p.spec
        .quotas
        .entry(resource.to_owned())
        .or_insert(default_limit);
    p.status.usage.insert(resource.to_owned(), used);
    gate::check_quota_in(&p, resource, 1).map_err(KeepError::Denied)
}

/// Mirror the counted usage into the project's status. Best effort: a
/// missing project is skipped, a failed write is logged.
pub(crate) fn record(store: &Store, project: &Id, resource: &str, used: u64) {
    let col = store.collection::<gate::Project>();
    match col.try_get(project) {
        Ok(Some(p)) if p.status.usage.get(resource) == Some(&used) => {}
        Ok(Some(_)) => {
            if let Err(e) = col.modify(project, |p| {
                p.status.usage.insert(resource.to_owned(), used);
                Ok(())
            }) {
                tracing::warn!(%project, resource, error = %e, "keep: usage was not recorded");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(%project, resource, error = %e, "keep: usage was not recorded"),
    }
}

/// `quota_exceeded` for a cloud-wide limit.
pub(crate) fn cloud_limit(resource: &str, used: u64, limit: u64, hint: &str) -> KeepError {
    KeepError::Denied(
        ApiError::quota(format!(
            "limit reached for {resource}: {used} of {limit} in use; {hint}"
        ))
        .with_detail("resource", resource)
        .with_detail("limit", limit)
        .with_detail("used", used),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_and_overrides() {
        assert_eq!(
            KeepConfig::default().max_secrets_per_project,
            DEFAULT_MAX_SECRETS_PER_PROJECT
        );
        let cfg = pistack_config::Config::from_toml(
            "[modules.keep]\nmax_secrets_per_project = 5\nmax_join_token_uses = 3\n",
        )
        .unwrap();
        let k = KeepConfig::from_config(&cfg).unwrap();
        assert_eq!(k.max_secrets_per_project, 5);
        assert_eq!(k.max_join_token_uses, 3);
        assert_eq!(
            k.max_certificates_per_project,
            DEFAULT_MAX_CERTIFICATES_PER_PROJECT
        );
        let bad = pistack_config::Config::from_toml("[modules.keep]\nmax_secret = 5\n").unwrap();
        assert!(KeepConfig::from_config(&bad).is_err());
        let zero =
            pistack_config::Config::from_toml("[modules.keep]\nmax_join_token_uses = 0\n").unwrap();
        assert!(KeepConfig::from_config(&zero).is_err());
    }

    #[test]
    fn join_token_grace_defaults_to_a_week_and_can_be_set() {
        let d = KeepConfig::default();
        assert_eq!(d.join_token_grace_secs, 7 * 24 * 3600);
        assert_eq!(
            d.join_token_grace(),
            std::time::Duration::from_secs(604_800)
        );
        let cfg = pistack_config::Config::from_toml(
            "[modules.keep]
join_token_grace_secs = 0
",
        )
        .unwrap();
        assert_eq!(
            KeepConfig::from_config(&cfg).unwrap().join_token_grace_secs,
            0
        );
    }
}
