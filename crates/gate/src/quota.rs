// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Quotas. A project's `spec.quotas` holds hard limits per resource name
//! (`"machines"`, `"vcpus"`, `"volumes_gib"`, ...); its `status.usage` holds
//! what is currently consumed. Other modules call `check_quota` before
//! creating something and `record_usage` after they did (or freed it).

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};

use crate::types::Project;

/// Would adding `delta` units of `resource` to `project` exceed its quota?
///
/// No limit configured means unlimited. A disabled project rejects any
/// positive delta.
pub fn check_quota(store: &Store, project: &Id, resource: &str, delta: u64) -> ApiResult<()> {
    let proj = store.collection::<Project>().get(project)?;
    check_quota_in(&proj, resource, delta)
}

/// Pure form of `check_quota` for a project already in hand.
pub fn check_quota_in(project: &Project, resource: &str, delta: u64) -> ApiResult<()> {
    if delta == 0 {
        return Ok(());
    }
    if !project.spec.enabled {
        return Err(ApiError::forbidden(format!(
            "project '{}' is disabled",
            project.meta.name
        )));
    }
    let Some(&limit) = project.spec.quotas.get(resource) else {
        return Ok(());
    };
    let used = project.status.usage.get(resource).copied().unwrap_or(0);
    let wanted = used.saturating_add(delta);
    if wanted > limit {
        return Err(ApiError::quota(format!(
            "quota exceeded for {resource} in project '{}': {used} used + {delta} requested > {limit}",
            project.meta.name
        ))
        .with_detail("resource", resource)
        .with_detail("limit", limit)
        .with_detail("used", used)
        .with_detail("requested", delta));
    }
    Ok(())
}

/// Adjust recorded usage of `resource` by `delta` (negative to release).
/// Usage never goes below zero. Returns the new usage.
pub fn record_usage(store: &Store, project: &Id, resource: &str, delta: i64) -> ApiResult<u64> {
    let updated = store.collection::<Project>().modify(project, |p| {
        let cur = p.status.usage.entry(resource.to_owned()).or_insert(0);
        *cur = if delta >= 0 {
            cur.saturating_add(delta as u64)
        } else {
            cur.saturating_sub(delta.unsigned_abs())
        };
        Ok(())
    })?;
    Ok(updated.status.usage.get(resource).copied().unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ProjectSpec, ProjectStatus};
    use pistack_types::{error::ErrorCode, kind, Meta};

    fn project(limit: Option<u64>, used: u64) -> Project {
        let mut p = Project {
            meta: Meta::new(kind::PROJECT, "p", None),
            spec: ProjectSpec::default(),
            status: ProjectStatus::default(),
        };
        if let Some(l) = limit {
            p.spec.quotas.insert("machines".into(), l);
        }
        if used > 0 {
            p.status.usage.insert("machines".into(), used);
        }
        p
    }

    #[test]
    fn limits() {
        assert!(check_quota_in(&project(None, 1000), "machines", 5).is_ok());
        assert!(check_quota_in(&project(Some(10), 8), "machines", 2).is_ok());
        let e = check_quota_in(&project(Some(10), 8), "machines", 3).unwrap_err();
        assert_eq!(e.code, ErrorCode::QuotaExceeded);
        assert_eq!(e.details["limit"], 10);
        let mut off = project(Some(10), 0);
        off.spec.enabled = false;
        assert!(check_quota_in(&off, "machines", 1).is_err());
        assert!(check_quota_in(&off, "machines", 0).is_ok());
    }

    #[test]
    fn usage_tracks_through_store() {
        let store = Store::in_memory().unwrap();
        let p = store
            .collection::<Project>()
            .create(project(Some(2), 0))
            .unwrap();
        assert_eq!(record_usage(&store, &p.meta.id, "machines", 2).unwrap(), 2);
        assert!(check_quota(&store, &p.meta.id, "machines", 1).is_err());
        assert_eq!(record_usage(&store, &p.meta.id, "machines", -5).unwrap(), 0);
        assert!(check_quota(&store, &p.meta.id, "machines", 1).is_ok());
    }
}
