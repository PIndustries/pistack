// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Compute quotas: `machines`, `vcpus`, `memory_mib` and `disk_gib` per
//! project. No project, or no limit for a resource, means unlimited. A
//! disabled project rejects a create.
//!
//! Usage is counted from the machine rows at each admission: every machine
//! of the project whose delete has not been accepted, with the resources of
//! the size it names. A size that no longer exists counts as nothing. The
//! count, the check and the write of the row run under gate's admission
//! lock, so two creates cannot both take the last unit.
//!
//! A pod's machine is counted like any other, but the pod controller
//! stores it without asking here, so a pod is not refused.

use gate::quota::{self, Demand, DISK_GIB, MACHINES, MEMORY_MIB, VCPUS};
use pistack_store::Store;
use pistack_types::{ApiResult, Id, Resources};
use std::collections::BTreeMap;

use crate::types::{Machine, Size, DELETE_ANNOTATION};

/// What a project's machines add up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub machines: u64,
    pub vcpus: u64,
    pub memory_mib: u64,
    pub disk_gib: u64,
}

impl Usage {
    fn add(&mut self, r: &Resources) {
        self.machines += 1;
        self.vcpus += u64::from(r.vcpus);
        self.memory_mib += r.memory_mib;
        self.disk_gib += r.disk_gib;
    }
}

/// Count the project's machines and what their sizes add up to.
pub fn count(store: &Store, project: &Id) -> ApiResult<Usage> {
    let sizes: BTreeMap<String, Resources> = store
        .collection::<Size>()
        .list(None)?
        .into_iter()
        .map(|s| (s.meta.name.clone(), s.spec.resources))
        .collect();
    let mut usage = Usage::default();
    for m in store.collection::<Machine>().list(Some(project))? {
        if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
            continue;
        }
        let r = sizes.get(&m.spec.size).copied().unwrap_or_default();
        usage.add(&r);
    }
    Ok(usage)
}

/// Store `machine`, whose size has `resources`, unless that would take its
/// project over a limit.
pub fn create(store: &Store, machine: Machine, resources: &Resources) -> ApiResult<Machine> {
    let Some(project) = machine.meta.project.clone() else {
        return Ok(store.collection::<Machine>().create(machine)?);
    };
    let _admission = quota::admission();
    let used = count(store, &project)?;
    quota::admit(
        store,
        &project,
        &[
            Demand::new(MACHINES, used.machines, 1),
            Demand::new(VCPUS, used.vcpus, u64::from(resources.vcpus)),
            Demand::new(MEMORY_MIB, used.memory_mib, resources.memory_mib),
            Demand::new(DISK_GIB, used.disk_gib, resources.disk_gib),
        ],
    )?;
    let machine = store.collection::<Machine>().create(machine)?;
    mirror(store, &project);
    Ok(machine)
}

/// Run `write`, which moves a machine of `project` from a size with `from`
/// to one with `to`, unless what the new size adds would go over a limit.
pub fn resize<T>(
    store: &Store,
    project: Option<&Id>,
    from: &Resources,
    to: &Resources,
    write: impl FnOnce() -> ApiResult<T>,
) -> ApiResult<T> {
    let Some(project) = project else {
        return write();
    };
    let _admission = quota::admission();
    let used = count(store, project)?;
    let more = |to: u64, from: u64| to.saturating_sub(from);
    quota::admit(
        store,
        project,
        &[
            Demand::new(
                VCPUS,
                used.vcpus,
                more(u64::from(to.vcpus), u64::from(from.vcpus)),
            ),
            Demand::new(
                MEMORY_MIB,
                used.memory_mib,
                more(to.memory_mib, from.memory_mib),
            ),
            Demand::new(DISK_GIB, used.disk_gib, more(to.disk_gib, from.disk_gib)),
        ],
    )?;
    let out = write()?;
    mirror(store, project);
    Ok(out)
}

/// Record the project's usage after a machine's delete was accepted.
pub fn note_delete(store: &Store, project: Option<&Id>) {
    let Some(project) = project else {
        return;
    };
    let _admission = quota::admission();
    mirror(store, project);
}

/// Count again and copy the result to the project. Admission lock held.
fn mirror(store: &Store, project: &Id) {
    match count(store, project) {
        Ok(u) => quota::mirror_usage(
            store,
            project,
            &[
                (MACHINES, u.machines),
                (VCPUS, u.vcpus),
                (MEMORY_MIB, u.memory_mib),
                (DISK_GIB, u.disk_gib),
            ],
        ),
        Err(e) => tracing::warn!(%project, error = %e, "forge: usage was not recorded"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{new_machine, new_size, MachineSpec};
    use chrono::Utc;
    use gate::{Project, ProjectSpec, ProjectStatus};
    use pistack_types::error::ErrorCode;
    use pistack_types::{kind, Meta};

    const SMALL: Resources = Resources {
        vcpus: 1,
        memory_mib: 1024,
        disk_gib: 10,
        gpus: 0,
    };
    const BIG: Resources = Resources {
        vcpus: 4,
        memory_mib: 8192,
        disk_gib: 40,
        gpus: 0,
    };

    fn store() -> Store {
        let store = Store::in_memory().unwrap();
        let sizes = store.collection::<Size>();
        sizes.create(new_size("small", SMALL)).unwrap();
        sizes.create(new_size("big", BIG)).unwrap();
        store
    }

    fn tenant(store: &Store, name: &str, limits: &[(&str, u64)]) -> Id {
        let mut spec = ProjectSpec::default();
        for (r, n) in limits {
            spec.quotas.insert((*r).into(), *n);
        }
        store
            .collection::<Project>()
            .create(Project {
                meta: Meta::new(kind::PROJECT, name, None),
                spec,
                status: ProjectStatus::default(),
            })
            .unwrap()
            .meta
            .id
    }

    fn used(store: &Store, project: &Id, resource: &str) -> u64 {
        store
            .collection::<Project>()
            .get(project)
            .unwrap()
            .status
            .usage
            .get(resource)
            .copied()
            .unwrap_or(0)
    }

    fn boot(store: &Store, project: &Id, name: &str, size: &str) -> ApiResult<Machine> {
        let spec = MachineSpec {
            size: size.into(),
            image: "ubuntu".into(),
            ..Default::default()
        };
        let resources = if size == "big" { BIG } else { SMALL };
        create(
            store,
            new_machine(name, Some(project.clone()), spec),
            &resources,
        )
    }

    /// What `DELETE /machines/{id}` does once it accepts the delete.
    fn delete(store: &Store, m: &Machine) {
        store
            .collection::<Machine>()
            .modify(&m.meta.id, |x| {
                x.meta
                    .annotations
                    .insert(DELETE_ANNOTATION.into(), Utc::now().to_rfc3339());
                Ok(())
            })
            .unwrap();
        note_delete(store, m.meta.project.as_ref());
    }

    fn refused(e: &pistack_types::ApiError, resource: &str, used: u64, asked: u64, limit: u64) {
        assert_eq!(e.code, ErrorCode::QuotaExceeded);
        assert_eq!(
            e.message,
            format!(
                "quota exceeded for {resource} in project 'tight': \
                 {used} used + {asked} requested > {limit}"
            )
        );
        assert_eq!(e.details["resource"], resource);
        assert_eq!(e.details["limit"], limit);
        assert_eq!(e.details["used"], used);
        assert_eq!(e.details["requested"], asked);
    }

    #[test]
    fn a_full_project_refuses_and_delete_releases() {
        let store = store();
        let p = tenant(&store, "tight", &[(MACHINES, 1)]);
        let first = boot(&store, &p, "a", "small").unwrap();
        assert_eq!(used(&store, &p, MACHINES), 1);
        let e = boot(&store, &p, "b", "small").unwrap_err();
        refused(&e, MACHINES, 1, 1, 1);
        assert_eq!(
            store.collection::<Machine>().list(Some(&p)).unwrap().len(),
            1
        );

        delete(&store, &first);
        assert_eq!(used(&store, &p, MACHINES), 0);
        boot(&store, &p, "b", "small").unwrap();
        assert_eq!(used(&store, &p, MACHINES), 1);
    }

    #[test]
    fn vcpus_are_limited_in_total() {
        let store = store();
        let p = tenant(&store, "tight", &[(VCPUS, 5)]);
        let big = boot(&store, &p, "a", "big").unwrap();
        boot(&store, &p, "b", "small").unwrap();
        assert_eq!(used(&store, &p, VCPUS), 5);
        let e = boot(&store, &p, "c", "small").unwrap_err();
        refused(&e, VCPUS, 5, 1, 5);

        delete(&store, &big);
        assert_eq!(used(&store, &p, VCPUS), 1);
        boot(&store, &p, "c", "big").unwrap();
        assert_eq!(used(&store, &p, VCPUS), 5);
    }

    #[test]
    fn memory_is_limited_in_total() {
        let store = store();
        let p = tenant(&store, "tight", &[(MEMORY_MIB, 8192)]);
        let small = boot(&store, &p, "a", "small").unwrap();
        let e = boot(&store, &p, "b", "big").unwrap_err();
        refused(&e, MEMORY_MIB, 1024, 8192, 8192);

        delete(&store, &small);
        boot(&store, &p, "b", "big").unwrap();
        assert_eq!(used(&store, &p, MEMORY_MIB), 8192);
        assert!(boot(&store, &p, "c", "small").is_err());
    }

    #[test]
    fn root_disk_is_limited_in_total() {
        let store = store();
        let p = tenant(&store, "tight", &[(DISK_GIB, 50)]);
        let big = boot(&store, &p, "a", "big").unwrap();
        boot(&store, &p, "b", "small").unwrap();
        let e = boot(&store, &p, "c", "small").unwrap_err();
        refused(&e, DISK_GIB, 50, 10, 50);

        delete(&store, &big);
        assert_eq!(used(&store, &p, DISK_GIB), 10);
        boot(&store, &p, "c", "small").unwrap();
    }

    #[test]
    fn no_limit_is_unlimited_and_usage_is_still_shown() {
        let store = store();
        let p = tenant(&store, "open", &[]);
        for i in 0..5 {
            boot(&store, &p, &format!("m{i}"), "big").unwrap();
        }
        assert_eq!(used(&store, &p, MACHINES), 5);
        assert_eq!(used(&store, &p, VCPUS), 20);
        assert_eq!(used(&store, &p, MEMORY_MIB), 5 * 8192);
        assert_eq!(used(&store, &p, DISK_GIB), 200);

        // No project at all is not limited either.
        let spec = MachineSpec {
            size: "big".into(),
            image: "ubuntu".into(),
            ..Default::default()
        };
        create(&store, new_machine("loose", None, spec), &BIG).unwrap();
    }

    #[test]
    fn a_disabled_project_refuses_a_create() {
        let store = store();
        let p = tenant(&store, "off", &[]);
        store
            .collection::<Project>()
            .modify(&p, |x| {
                x.spec.enabled = false;
                Ok(())
            })
            .unwrap();
        let e = boot(&store, &p, "a", "small").unwrap_err();
        assert_eq!(e.code, ErrorCode::Forbidden);
    }

    #[test]
    fn a_larger_size_is_checked_and_a_smaller_one_is_not() {
        let store = store();
        let p = tenant(&store, "tight", &[(VCPUS, 4)]);
        let m = boot(&store, &p, "a", "small").unwrap();
        let other = boot(&store, &p, "b", "small").unwrap();
        let to = |size: &'static str| {
            let id = m.meta.id.clone();
            let store = store.clone();
            move || -> ApiResult<Machine> {
                Ok(store.collection::<Machine>().modify(&id, |x| {
                    x.spec.size = size.into();
                    Ok(())
                })?)
            }
        };

        let e = resize(&store, Some(&p), &SMALL, &BIG, to("big")).unwrap_err();
        refused(&e, VCPUS, 2, 3, 4);
        assert_eq!(
            store
                .collection::<Machine>()
                .get(&m.meta.id)
                .unwrap()
                .spec
                .size,
            "small"
        );

        delete(&store, &other);
        resize(&store, Some(&p), &SMALL, &BIG, to("big")).unwrap();
        assert_eq!(used(&store, &p, VCPUS), 4);
        resize(&store, Some(&p), &BIG, &SMALL, to("small")).unwrap();
        assert_eq!(used(&store, &p, VCPUS), 1);
    }

    #[test]
    fn concurrent_creates_stop_at_the_limit() {
        let store = store();
        let p = tenant(&store, "tight", &[(VCPUS, 3)]);
        let threads: Vec<_> = (0..12)
            .map(|i| {
                let (store, p) = (store.clone(), p.clone());
                std::thread::spawn(move || boot(&store, &p, &format!("m{i}"), "small").is_ok())
            })
            .collect();
        let made = threads
            .into_iter()
            .map(|t| t.join().unwrap())
            .filter(|ok| *ok)
            .count();
        assert_eq!(made, 3);
        assert_eq!(count(&store, &p).unwrap().vcpus, 3);
        assert_eq!(used(&store, &p, VCPUS), 3);
    }
}
