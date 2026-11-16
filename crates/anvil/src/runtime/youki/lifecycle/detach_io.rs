// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Stdio for `create` and `start`. The container init inherits these fds, so
/// none of them is a pipe. `None` is `/dev/null` (the sandbox pause process).
use std::path::PathBuf;

pub struct DetachIo {
    pub(in crate::runtime::youki) stdout: Option<PathBuf>,
    pub(in crate::runtime::youki) stderr: Option<PathBuf>,
    /// Agent-created file passed as `youki --log`. Read after youki exits.
    pub(in crate::runtime::youki) youki_log: PathBuf,
}
