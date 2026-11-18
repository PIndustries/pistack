// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `2 vcpus, 4096 MiB, 20 GiB disk`.
pub(crate) fn room(r: &pistack_types::Resources) -> String {
    format!(
        "{} vcpus, {} MiB, {} GiB disk",
        r.vcpus, r.memory_mib, r.disk_gib
    )
}
