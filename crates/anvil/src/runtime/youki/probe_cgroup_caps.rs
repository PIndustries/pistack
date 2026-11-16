// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(target_os = "linux")]
use crate::runtime::youki::cgroups::cgroup_v2_dir;
#[cfg(target_os = "linux")]
use crate::runtime::youki::cgroups::dir_writable;
use crate::runtime::youki::privilege::CgroupCaps;

pub(super) fn probe_cgroup_caps() -> CgroupCaps {
    #[cfg(target_os = "linux")]
    {
        let Some(dir) = cgroup_v2_dir() else {
            return CgroupCaps {
                memory: false,
                cpu: false,
            };
        };
        if !dir.is_dir() || !dir_writable(&dir) {
            return CgroupCaps {
                memory: false,
                cpu: false,
            };
        }
        let Ok(text) = std::fs::read_to_string(dir.join("cgroup.controllers")) else {
            return CgroupCaps {
                memory: false,
                cpu: false,
            };
        };
        let has = |name: &str| text.split_whitespace().any(|c| c == name);
        CgroupCaps {
            memory: has("memory"),
            cpu: has("cpu"),
        }
    }
    #[cfg(not(target_os = "linux"))]
    {
        CgroupCaps {
            memory: false,
            cpu: false,
        }
    }
}
