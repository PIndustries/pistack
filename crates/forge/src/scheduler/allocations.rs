// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Id, Resources};
use std::collections::BTreeMap;

/// Resources and machine names the scheduler should treat as already taken,
/// on top of what `Host.status.free` reports. The reconciler accumulates
/// these while placing several machines in one pass.
#[derive(Clone, Debug, Default)]
pub struct Allocations {
    pub(crate) used: BTreeMap<Id, Resources>,
    pub(crate) machines: BTreeMap<Id, Vec<String>>,
}

impl Allocations {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that `machine_name` consuming `res` lives on `host`.
    pub fn add(&mut self, host: &Id, machine_name: &str, res: Resources) {
        let cur = self.used.entry(host.clone()).or_default();
        *cur = cur.add(&res);
        self.machines
            .entry(host.clone())
            .or_default()
            .push(machine_name.to_owned());
    }

    pub fn used_on(&self, host: &Id) -> Resources {
        self.used.get(host).copied().unwrap_or_default()
    }

    pub fn machines_on(&self, host: &Id) -> &[String] {
        self.machines.get(host).map(Vec::as_slice).unwrap_or(&[])
    }
}
