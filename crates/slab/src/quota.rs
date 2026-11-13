// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The quota hook. `Client` asks it before it stores a volume, a snapshot
//! or a backup, and before it grows a volume, with gate's admission lock
//! held from the question to the write. [`ProjectQuota`] is the default and
//! checks the project's limits in `gate`. [`AllowAll`] turns the check off.

use gate::quota::{self, Demand};
use pistack_store::Store;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiResult, Id};

/// What a request would consume. `resource` is the gate quota name.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QuotaRequest {
    /// `volumes`, `volumes_gib`, `snapshots`, `backups`, `backups_gib`.
    pub resource: &'static str,
    pub amount: u64,
}

impl QuotaRequest {
    pub fn volumes(n: u64) -> Self {
        Self {
            resource: quota::VOLUMES,
            amount: n,
        }
    }
    /// Volume capacity, in GiB.
    pub fn gigabytes(n: u64) -> Self {
        Self {
            resource: quota::VOLUMES_GIB,
            amount: n,
        }
    }
    pub fn snapshots(n: u64) -> Self {
        Self {
            resource: quota::SNAPSHOTS,
            amount: n,
        }
    }
    pub fn backups(n: u64) -> Self {
        Self {
            resource: quota::BACKUPS,
            amount: n,
        }
    }
    /// Backup capacity, in GiB of the volume backed up.
    pub fn backup_gigabytes(n: u64) -> Self {
        Self {
            resource: quota::BACKUPS_GIB,
            amount: n,
        }
    }
}

pub trait QuotaCheck: Send + Sync + 'static {
    /// Return `Err(ApiError::quota(..))` to refuse. `project` is `None` for
    /// cloud-scoped requests, which are never quota-limited. Called with
    /// gate's admission lock held, so it must not take it.
    fn check(
        &self,
        store: &Store,
        project: Option<&Id>,
        requests: &[QuotaRequest],
    ) -> ApiResult<()>;
}

/// The project's limits in `gate`, against usage counted from slab's rows.
/// No limit for a resource means unlimited, and so does a project gate has
/// no row for. A disabled project refuses.
#[derive(Default, Clone, Copy, Debug)]
pub struct ProjectQuota;

impl QuotaCheck for ProjectQuota {
    fn check(
        &self,
        store: &Store,
        project: Option<&Id>,
        requests: &[QuotaRequest],
    ) -> ApiResult<()> {
        let Some(project) = project else {
            return Ok(());
        };
        let used = crate::usage::count(store, project)?;
        let demands: Vec<Demand<'_>> = requests
            .iter()
            .map(|r| Demand::new(r.resource, used.of(r.resource), r.amount))
            .collect();
        match quota::admit(store, project, &demands) {
            Err(e) if e.code == ErrorCode::NotFound => Ok(()),
            other => other,
        }
    }
}

/// Everything is allowed.
#[derive(Default, Clone, Copy, Debug)]
pub struct AllowAll;

impl QuotaCheck for AllowAll {
    fn check(
        &self,
        _store: &Store,
        _project: Option<&Id>,
        _requests: &[QuotaRequest],
    ) -> ApiResult<()> {
        Ok(())
    }
}
