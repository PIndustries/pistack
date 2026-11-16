// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(super) fn agent_ids() -> (u32, u32) {
    #[cfg(test)]
    if let Some(ids) = test_agent_ids() {
        return ids;
    }
    #[cfg(unix)]
    {
        // geteuid and getegid do not fail.
        let uid = unsafe { libc::geteuid() };
        let gid = unsafe { libc::getegid() };
        (uid, gid)
    }
    #[cfg(not(unix))]
    (0, 0)
}

#[cfg(test)]
pub(in crate::runtime::youki) fn test_agent_ids() -> Option<(u32, u32)> {
    let uid = std::env::var("PISTACK_YOUKI_TEST_EUID")
        .ok()
        .and_then(|v| v.parse().ok())?;
    let gid = std::env::var("PISTACK_YOUKI_TEST_EGID")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(uid);
    Some((uid, gid))
}
