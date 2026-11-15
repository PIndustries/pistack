// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::agent_ids::agent_ids;
use crate::runtime::youki::consts::SUDOERS_SAMPLE;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::env_flag::env_flag;
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege_unavailable::privilege_unavailable;
use crate::runtime::youki::sudo_bin::sudo_bin;
use crate::runtime::youki::unprivileged_userns_available::unprivileged_userns_available;
use pistack_types::ApiError;
use pistack_types::ApiResult;

pub(super) fn resolve_privilege() -> ApiResult<Privilege> {
    let (uid, gid) = agent_ids();
    if uid == 0 {
        return Ok(Privilege::Root);
    }
    if env_flag(SUDO_ENV) {
        return match sudo_bin() {
            Some(sudo) => Ok(Privilege::Sudo { sudo }),
            None if unprivileged_userns_available() => Err(ApiError::unavailable(format!(
                "youki runtime: {SUDO_ENV} is set but sudo is not available. Install sudo and a NOPASSWD sudoers rule for the youki binary, the cleanup helper, ip, and ovs-vsctl (`{SUDOERS_SAMPLE}`), or unset {SUDO_ENV} to use a rootless user namespace"
            ))),
            None => Err(privilege_unavailable()),
        };
    }
    if unprivileged_userns_available() {
        return Ok(Privilege::Rootless { uid, gid });
    }
    Err(privilege_unavailable())
}
