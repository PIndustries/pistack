// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::path::{Path, PathBuf};

use crate::nanoid::nanoid;

/// A password file for the length of one command.
pub(crate) enum PassFile {
    /// A file that outlives this command; not removed.
    Borrowed(PathBuf),
    /// A temporary file this guard removes on drop.
    Temp(PathBuf),
}

impl PassFile {
    pub(crate) fn from_bytes(bytes: &[u8]) -> ApiResult<PassFile> {
        let dir = std::env::temp_dir().join(format!("pistack-power-{}", std::process::id()));
        std::fs::create_dir_all(&dir).map_err(|e| {
            ApiError::unavailable(format!("power secret dir {}: {e}", dir.display()))
        })?;
        let path = dir.join(format!("pass-{}", nanoid()));
        std::fs::write(&path, bytes)
            .map_err(|e| ApiError::unavailable(format!("power secret file: {e}")))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(PassFile::Temp(path))
    }

    pub(crate) fn path(&self) -> ApiResult<&Path> {
        match self {
            PassFile::Borrowed(p) | PassFile::Temp(p) => Ok(p.as_path()),
        }
    }
}

impl Drop for PassFile {
    fn drop(&mut self) {
        if let PassFile::Temp(path) = self {
            let _ = std::fs::remove_file(path);
        }
    }
}
