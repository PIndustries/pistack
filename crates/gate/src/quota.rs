// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Quotas. A project's `spec.quotas` holds hard limits per resource name
//! ([`RESOURCES`]); a name with no entry is unlimited. Its `status.usage`
//! shows what is consumed.
//!
//! The module that owns a resource counts usage from its own stored rows
//! each time it admits a request, so the number checked cannot drift from
//! what exists. `status.usage` is a copy of that count kept for reading.
//!
//! Count, check and the write that consumes the resource run under one
//! process-wide lock ([`admission`]):
//!
//! ```text
//! let _admission = gate::quota::admission();
//! gate::quota::admit(store, project, &[Demand::new("volumes", used, 1)])?;
//! // store the row
//! gate::quota::mirror_usage(store, project, &[("volumes", used + 1)]);
//! ```
//!
//! Two requests in one process therefore cannot both take the last unit.
//! Only the leader accepts writes, so in steady state that is the whole
//! cluster. The store has no transaction that spans a count and an insert,
//! so a leader that has not yet learned it was replaced can admit against
//! its own copy; `keep`'s `quota.rs` works the bound out.

use std::sync::{Mutex, MutexGuard};

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id};

use crate::types::Project;

pub const MACHINES: &str = "machines";
pub const VCPUS: &str = "vcpus";
pub const MEMORY_MIB: &str = "memory_mib";
pub const DISK_GIB: &str = "disk_gib";
pub const VOLUMES: &str = "volumes";
pub const VOLUMES_GIB: &str = "volumes_gib";
pub const SNAPSHOTS: &str = "snapshots";
pub const BACKUPS: &str = "backups";
pub const BACKUPS_GIB: &str = "backups_gib";
pub const NETWORKS: &str = "networks";
pub const SUBNETS: &str = "subnets";
pub const ROUTERS: &str = "routers";
pub const SECURITY_GROUPS: &str = "security_groups";
pub const PORTS: &str = "ports";
pub const FLOATING_IPS: &str = "floating_ips";
pub const IMAGES: &str = "images";
pub const IMAGES_BYTES: &str = "images_bytes";
pub const BUCKETS: &str = "buckets";
pub const SECRETS: &str = "secrets";
pub const CERTIFICATES: &str = "certificates";
pub const ACCESS_KEYS: &str = "access_keys";

/// Every resource name some module enforces.
pub const RESOURCES: &[&str] = &[
    MACHINES,
    VCPUS,
    MEMORY_MIB,
    DISK_GIB,
    VOLUMES,
    VOLUMES_GIB,
    SNAPSHOTS,
    BACKUPS,
    BACKUPS_GIB,
    NETWORKS,
    SUBNETS,
    ROUTERS,
    SECURITY_GROUPS,
    PORTS,
    FLOATING_IPS,
    IMAGES,
    IMAGES_BYTES,
    BUCKETS,
    SECRETS,
    CERTIFICATES,
    ACCESS_KEYS,
];

/// `requested` more of `resource` on top of `used`, which the caller has
/// just counted.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Demand<'a> {
    pub resource: &'a str,
    pub used: u64,
    pub requested: u64,
}

impl<'a> Demand<'a> {
    pub fn new(resource: &'a str, used: u64, requested: u64) -> Self {
        Self {
            resource,
            used,
            requested,
        }
    }
}

/// Held from the count to the write it allows. Never held across an await.
pub fn admission() -> MutexGuard<'static, ()> {
    static LOCK: Mutex<()> = Mutex::new(());
    // Nothing is behind the lock, so a panic under it leaves nothing broken.
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

/// Refuse unless every demand fits under the project's limit. Call with
/// [`admission`] held. A project that does not exist is `not_found`.
pub fn admit(store: &Store, project: &Id, demands: &[Demand<'_>]) -> ApiResult<()> {
    let proj = store.collection::<Project>().get(project)?;
    admit_in(&proj, demands)
}

/// [`admit`] for a project already in hand. No limit configured means
/// unlimited. A disabled project refuses anything that asks for more.
pub fn admit_in(project: &Project, demands: &[Demand<'_>]) -> ApiResult<()> {
    for d in demands {
        if d.requested == 0 {
            continue;
        }
        if !project.spec.enabled {
            return Err(ApiError::forbidden(format!(
                "project '{}' is disabled",
                project.meta.name
            )));
        }
        let Some(&limit) = project.spec.quotas.get(d.resource) else {
            continue;
        };
        if d.used.saturating_add(d.requested) > limit {
            return Err(ApiError::quota(format!(
                "quota exceeded for {} in project '{}': {} used + {} requested > {limit}",
                d.resource, project.meta.name, d.used, d.requested
            ))
            .with_detail("resource", d.resource)
            .with_detail("limit", limit)
            .with_detail("used", d.used)
            .with_detail("requested", d.requested));
        }
    }
    Ok(())
}

/// Would adding `delta` to the usage recorded in `project` exceed its
/// limit? For a caller that has put its own count in `status.usage`.
pub fn check_quota_in(project: &Project, resource: &str, delta: u64) -> ApiResult<()> {
    let used = project.status.usage.get(resource).copied().unwrap_or(0);
    admit_in(project, &[Demand::new(resource, used, delta)])
}

/// Copy counted usage into the project's `status.usage`. Best effort: the
/// count that is enforced comes from the rows, so a missing project is
/// skipped and a failed write is only logged.
pub fn mirror_usage(store: &Store, project: &Id, counted: &[(&str, u64)]) {
    let col = store.collection::<Project>();
    let stale = |p: &Project| {
        counted
            .iter()
            .any(|(r, n)| p.status.usage.get(*r).copied().unwrap_or(0) != *n)
    };
    match col.try_get(project) {
        Ok(Some(p)) if !stale(&p) => {}
        Ok(Some(_)) => {
            if let Err(e) = col.modify(project, |p| {
                for (r, n) in counted {
                    p.status.usage.insert((*r).to_owned(), *n);
                }
                Ok(())
            }) {
                tracing::warn!(%project, error = %e, "gate: usage was not recorded");
            }
        }
        Ok(None) => {}
        Err(e) => tracing::warn!(%project, error = %e, "gate: usage was not recorded"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{ProjectSpec, ProjectStatus};
    use pistack_types::{error::ErrorCode, kind, Meta};

    fn project(limit: Option<u64>) -> Project {
        let mut p = Project {
            meta: Meta::new(kind::PROJECT, "p", None),
            spec: ProjectSpec::default(),
            status: ProjectStatus::default(),
        };
        if let Some(l) = limit {
            p.spec.quotas.insert(VCPUS.into(), l);
        }
        p
    }

    #[test]
    fn limits() {
        let ask = |limit, used, requested| {
            admit_in(&project(limit), &[Demand::new(VCPUS, used, requested)])
        };
        assert!(ask(None, 1000, 5).is_ok());
        assert!(ask(Some(10), 8, 2).is_ok());
        let e = ask(Some(10), 8, 3).unwrap_err();
        assert_eq!(e.code, ErrorCode::QuotaExceeded);
        assert_eq!(
            e.message,
            "quota exceeded for vcpus in project 'p': 8 used + 3 requested > 10"
        );
        assert_eq!(e.details["resource"], VCPUS);
        assert_eq!(e.details["limit"], 10);
        assert_eq!(e.details["used"], 8);
        assert_eq!(e.details["requested"], 3);
        // Already over, for instance after the limit was lowered: asking
        // for nothing is still fine.
        assert!(ask(Some(10), 12, 0).is_ok());
        assert!(ask(Some(10), 12, 1).is_err());

        let mut off = project(Some(10));
        off.spec.enabled = false;
        let e = admit_in(&off, &[Demand::new(VCPUS, 0, 1)]).unwrap_err();
        assert_eq!(e.code, ErrorCode::Forbidden);
        assert!(admit_in(&off, &[Demand::new(VCPUS, 0, 0)]).is_ok());
    }

    #[test]
    fn the_first_demand_over_its_limit_is_the_one_named() {
        let mut p = project(Some(4));
        p.spec.quotas.insert(MEMORY_MIB.into(), 1024);
        let e = admit_in(
            &p,
            &[
                Demand::new(MACHINES, 50, 1),
                Demand::new(VCPUS, 2, 2),
                Demand::new(MEMORY_MIB, 512, 1024),
            ],
        )
        .unwrap_err();
        assert_eq!(e.details["resource"], MEMORY_MIB);
    }

    #[test]
    fn recorded_usage_can_be_checked() {
        let mut p = project(Some(2));
        p.status.usage.insert(VCPUS.into(), 2);
        assert!(check_quota_in(&p, VCPUS, 1).is_err());
        assert!(check_quota_in(&p, MACHINES, 1).is_ok());
    }

    #[test]
    fn usage_is_mirrored_and_a_missing_project_is_skipped() {
        let store = Store::in_memory().unwrap();
        let p = store
            .collection::<Project>()
            .create(project(Some(2)))
            .unwrap();
        let id = p.meta.id;
        mirror_usage(&store, &id, &[(VCPUS, 2), (MACHINES, 1)]);
        let seen = store.collection::<Project>().get(&id).unwrap();
        assert_eq!(seen.status.usage[VCPUS], 2);
        assert_eq!(seen.status.usage[MACHINES], 1);
        // Unchanged counts do not write.
        mirror_usage(&store, &id, &[(VCPUS, 2)]);
        let again = store.collection::<Project>().get(&id).unwrap();
        assert_eq!(again.meta.revision, seen.meta.revision);

        assert!(admit(&store, &id, &[Demand::new(VCPUS, 2, 1)]).is_err());
        assert!(admit(&store, &id, &[Demand::new(VCPUS, 1, 1)]).is_ok());
        let gone = Id::new(kind::PROJECT);
        mirror_usage(&store, &gone, &[(VCPUS, 2)]);
        let e = admit(&store, &gone, &[Demand::new(VCPUS, 0, 1)]).unwrap_err();
        assert_eq!(e.code, ErrorCode::NotFound);
    }
}
