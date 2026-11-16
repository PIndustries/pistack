// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::privilege::CgroupCaps;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::probe_cgroup_caps::probe_cgroup_caps;
use crate::runtime::youki::resolve_privilege::resolve_privilege;
use pistack_types::ApiResult;

pub(super) fn resolve_run_mode() -> ApiResult<RunMode> {
    let privilege = resolve_privilege()?;
    let cgroup = match privilege {
        Privilege::Rootless { .. } => probe_cgroup_caps(),
        Privilege::Root | Privilege::Sudo { .. } => CgroupCaps {
            memory: true,
            cpu: true,
        },
    };
    Ok(RunMode { privilege, cgroup })
}
