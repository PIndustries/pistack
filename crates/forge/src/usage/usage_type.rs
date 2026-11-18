// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resources;

/// What a project's machines add up to.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub machines: u64,
    pub vcpus: u64,
    pub memory_mib: u64,
    pub disk_gib: u64,
}

impl Usage {
    pub(crate) fn add(&mut self, r: &Resources) {
        self.machines += 1;
        self.vcpus += u64::from(r.vcpus);
        self.memory_mib += r.memory_mib;
        self.disk_gib += r.disk_gib;
    }
}
