// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self, Demand, DISK_GIB, MEMORY_MIB, VCPUS};
use pistack_store::Store;
use pistack_types::{ApiResult, Id, Resources};

use super::count::count;
use super::mirror::mirror;

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
