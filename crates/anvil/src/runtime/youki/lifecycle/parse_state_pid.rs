// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn parse_state_pid(stdout: &[u8]) -> Option<u32> {
    let value: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let pid = value.get("pid")?.as_u64()?;
    u32::try_from(pid).ok().filter(|pid| *pid > 0)
}
