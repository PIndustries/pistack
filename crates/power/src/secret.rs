// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::path::{Path, PathBuf};

/// Where the BMC password comes from for one control.
#[derive(Clone, Debug)]
pub enum Secret {
    /// A file whose first line is the password. The tool reads it; the
    /// password never enters this process's memory as an argument.
    File(PathBuf),
    /// The password in memory, for a caller that read it from a secret
    /// service. Never shown, never logged.
    Bytes(Vec<u8>),
}

impl Secret {
    /// A file-based secret. A symlink or a non-file is refused.
    pub fn from_file(path: &Path) -> ApiResult<Secret> {
        let meta = std::fs::symlink_metadata(path)
            .map_err(|_| ApiError::unavailable("bmc password file is missing"))?;
        if meta.file_type().is_symlink() || !meta.is_file() {
            return Err(ApiError::unavailable("bmc password file is missing"));
        }
        Ok(Secret::File(path.to_path_buf()))
    }

    /// The password bytes, read from wherever this secret points. Never
    /// returned to a caller that would show them; used only to feed a tool.
    pub(crate) fn read(&self) -> ApiResult<Vec<u8>> {
        match self {
            Secret::File(path) => {
                let bytes = std::fs::read(path)
                    .map_err(|_| ApiError::unavailable("bmc password file is missing"))?;
                Ok(bytes)
            }
            Secret::Bytes(bytes) => Ok(bytes.clone()),
        }
    }

    /// The first non-empty line, trimmed, as the tools expect the password.
    pub(crate) fn password(&self) -> ApiResult<String> {
        let bytes = self.read()?;
        let text = String::from_utf8_lossy(&bytes);
        let line = text
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("");
        if line.is_empty() {
            return Err(ApiError::unavailable("bmc password is empty"));
        }
        Ok(line.to_owned())
    }
}
