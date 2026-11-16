// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::state_dir::state_dir;
use crate::runtime::youki::state_dir_error::state_dir_error;
use pistack_types::ApiResult;
use std::path::PathBuf;

pub(super) fn ensure_state_dir() -> ApiResult<PathBuf> {
    let dir = state_dir();
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return Err(state_dir_error(&dir, &e));
    }
    let probe = dir.join(".pistack-writable");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            Ok(dir)
        }
        Err(e) => Err(state_dir_error(&dir, &e)),
    }
}
