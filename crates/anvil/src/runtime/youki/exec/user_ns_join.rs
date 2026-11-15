// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Rootless container create/start must already be inside the sandbox user
/// namespace. youki 0.7 treats a joined user namespace (a path, no new map)
/// as "no user namespace" when the calling process is outside one.
use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use std::path::PathBuf;

pub(in crate::runtime::youki) fn user_ns_join(mode: &RunMode, pid: u32) -> Option<PathBuf> {
    match &mode.privilege {
        Privilege::Rootless { .. } => Some(PathBuf::from(format!("/proc/{pid}/ns/user"))),
        Privilege::Root | Privilege::Sudo { .. } => None,
    }
}
