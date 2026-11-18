// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self, Demand, DISK_GIB, MACHINES, MEMORY_MIB, VCPUS};
use pistack_store::Store;
use pistack_types::{ApiResult, Resources};

use crate::types::Machine;

use super::count::count;
use super::mirror::mirror;

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
