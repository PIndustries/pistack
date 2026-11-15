// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::chmod_writable;
use std::path::PathBuf;

pub struct TreeGuard(pub PathBuf);
impl Drop for TreeGuard {
    fn drop(&mut self) {
        chmod_writable(&self.0);
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
