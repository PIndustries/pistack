// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Best-effort cleanup of a staging path if the future is cancelled or errors
/// before rename. Call [`TmpGuard::disarm`] after a successful rename so Drop
/// does not attempt another delete of the (now gone) temp path.
pub(crate) struct TmpGuard(Option<std::path::PathBuf>);

impl TmpGuard {
    pub(crate) fn new(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }

    /// Stop Drop from deleting the path (use after a successful rename).
    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TmpGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}
