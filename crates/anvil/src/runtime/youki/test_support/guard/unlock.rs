// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;

pub struct Unlock(pub PathBuf);
impl Drop for Unlock {
    fn drop(&mut self) {
        if let Ok(mut perms) = std::fs::metadata(&self.0).map(|meta| meta.permissions()) {
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(&self.0, perms);
        }
    }
}
