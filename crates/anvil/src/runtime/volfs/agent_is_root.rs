// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::volfs) fn agent_is_root() -> bool {
    #[cfg(test)]
    if let Ok(raw) = std::env::var("PISTACK_VOLFS_TEST_EUID") {
        if let Ok(uid) = raw.parse::<u32>() {
            return uid == 0;
        }
    }
    #[cfg(unix)]
    {
        // geteuid does not fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}
