// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::sysctl_is_zero::sysctl_is_zero;

pub(super) fn unprivileged_userns_available() -> bool {
    #[cfg(test)]
    if let Ok(value) = std::env::var("PISTACK_YOUKI_TEST_USERNS") {
        return matches!(value.as_str(), "1" | "true" | "yes");
    }
    !sysctl_is_zero("/proc/sys/kernel/unprivileged_userns_clone")
        && !sysctl_is_zero("/proc/sys/user/max_user_namespaces")
}
