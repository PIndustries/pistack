// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use gate::quota::{self, DISK_GIB, MACHINES, MEMORY_MIB, VCPUS};
use pistack_store::Store;
use pistack_types::Id;

use super::count::count;

/// Count again and copy the result to the project. Admission lock held.
pub(crate) fn mirror(store: &Store, project: &Id) {
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
