// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Resources`: sizes used by forge, loom and the scheduler.

use serde::{Deserialize, Serialize};

/// Resource sizes used by forge, loom and the scheduler.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Default, Hash)]
pub struct Resources {
    pub vcpus: u32,
    pub memory_mib: u64,
    #[serde(default)]
    pub disk_gib: u64,
    #[serde(default)]
    pub gpus: u32,
}

impl Resources {
    pub fn fits_in(&self, free: &Resources) -> bool {
        self.vcpus <= free.vcpus
            && self.memory_mib <= free.memory_mib
            && self.disk_gib <= free.disk_gib
            && self.gpus <= free.gpus
    }
    pub fn saturating_sub(&self, other: &Resources) -> Resources {
        Resources {
            vcpus: self.vcpus.saturating_sub(other.vcpus),
            memory_mib: self.memory_mib.saturating_sub(other.memory_mib),
            disk_gib: self.disk_gib.saturating_sub(other.disk_gib),
            gpus: self.gpus.saturating_sub(other.gpus),
        }
    }
    pub fn add(&self, other: &Resources) -> Resources {
        Resources {
            vcpus: self.vcpus + other.vcpus,
            memory_mib: self.memory_mib + other.memory_mib,
            disk_gib: self.disk_gib + other.disk_gib,
            gpus: self.gpus + other.gpus,
        }
    }
}
