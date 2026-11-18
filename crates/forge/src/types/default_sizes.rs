// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resources;

use super::new_size::new_size;
use super::size::Size;

/// The sizes seeded on first start.
pub fn default_sizes() -> Vec<Size> {
    let r = |vcpus, memory_mib, disk_gib| Resources {
        vcpus,
        memory_mib,
        disk_gib,
        gpus: 0,
    };
    vec![
        new_size("s.1x1", r(1, 1024, 10)),
        new_size("m.2x4", r(2, 4096, 20)),
        new_size("l.4x8", r(4, 8192, 40)),
        new_size("xl.8x16", r(8, 16384, 80)),
    ]
}
