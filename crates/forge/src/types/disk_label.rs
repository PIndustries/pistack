// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Host label saying where the agent keeps machine root disks: `local`
/// when the directory is on this host's own filesystem, `shared` when it is
/// on a filesystem the other hosts mount too. The agent reports it and it
/// defaults to `local`, because that is what a host that never said is.
pub const DISK_LABEL: &str = "pistack.io/disk";
