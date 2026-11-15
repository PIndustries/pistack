// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::SUDOERS_SAMPLE;
use crate::runtime::youki::consts::SUDO_ENV;
use pistack_types::ApiError;

pub(super) fn privilege_unavailable() -> ApiError {
    ApiError::unavailable(format!(
        "youki runtime: the agent is not root, so youki needs {SUDO_ENV}=1 with a NOPASSWD sudoers rule for the youki binary, the cleanup helper, ip, and ovs-vsctl (`{SUDOERS_SAMPLE}`) or an unprivileged user namespace for a single-id map (no newuidmap). Run the agent as root, set {SUDO_ENV}=1, or enable unprivileged user namespaces"
    ))
}
