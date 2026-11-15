// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
use crate::runtime::youki::privilege::CgroupCaps;
use crate::runtime::youki::privilege::Privilege;

use crate::runtime::youki::privilege::RunMode;

impl RunMode {
    #[cfg(test)]
    pub(super) fn privileged() -> Self {
        Self {
            privilege: Privilege::Root,
            cgroup: CgroupCaps {
                memory: true,
                cpu: true,
            },
        }
    }

    #[cfg(test)]
    pub(super) fn rootless(uid: u32, gid: u32) -> Self {
        Self {
            privilege: Privilege::Rootless { uid, gid },
            cgroup: CgroupCaps {
                memory: false,
                cpu: false,
            },
        }
    }
}
