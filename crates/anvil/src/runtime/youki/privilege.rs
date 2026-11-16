// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// How this agent is allowed to run youki.
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub enum Privilege {
    /// Agent euid is 0. youki is executed directly with the root OCI config.
    Root,
    /// Agent is not root and `PISTACK_YOUKI_SUDO` is set. youki is
    /// `sudo -n <youki>` and the OCI config stays the root config.
    Sudo { sudo: PathBuf },
    /// Agent is not root and sudo mode is off. The OCI config creates a
    /// user namespace with a single-id map. No `newuidmap` binary is required.
    Rootless { uid: u32, gid: u32 },
}

/// Cgroup controllers this process may write. Root and sudo always report
/// both, because youki itself runs as root. Rootless reports a controller
/// only when the current cgroup directory is writable and lists it.
#[derive(Clone, Copy, Debug)]
pub struct CgroupCaps {
    pub(super) memory: bool,
    pub(super) cpu: bool,
}

#[derive(Clone, Debug)]
pub struct RunMode {
    pub(super) privilege: Privilege,
    pub(super) cgroup: CgroupCaps,
}
