// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Kills the youki process group if a wait is abandoned. Disarm after a
/// successful wait so a recycled pid is left alone.
use crate::runtime::youki::exec::signal_group;

pub struct GroupKill {
    pub(in crate::runtime::youki) pid: Option<u32>,
}

impl GroupKill {
    pub(in crate::runtime::youki) fn arm(pid: Option<u32>) -> Self {
        Self { pid }
    }

    pub(in crate::runtime::youki) fn disarm(&mut self) {
        self.pid = None;
    }
}

impl Drop for GroupKill {
    fn drop(&mut self) {
        if let Some(pid) = self.pid.take() {
            signal_group(pid);
        }
    }
}

impl GroupKill {}
