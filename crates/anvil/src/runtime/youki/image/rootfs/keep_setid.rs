// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Setuid and setgid stay when this agent is root or sudo mode is on.
/// Rootless unpack clears them so a layer cannot plant a privileged binary.
use crate::runtime::youki::agent_ids::agent_ids;
use crate::runtime::youki::consts::SUDO_ENV;
use crate::runtime::youki::env_flag::env_flag;

pub(in crate::runtime::youki) fn keep_setid() -> bool {
    let (uid, _) = agent_ids();
    uid == 0 || env_flag(SUDO_ENV)
}
