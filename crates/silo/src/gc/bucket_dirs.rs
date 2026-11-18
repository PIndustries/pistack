// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::io_err;
use pistack_types::ApiResult;
use std::path::Path;

/// Directories directly under `root` that can hold buckets (skips dot
/// entries such as `.tmp-*`). A missing root is empty.
pub(crate) async fn bucket_dirs(root: &Path) -> ApiResult<Vec<std::path::PathBuf>> {
    let mut entries = match tokio::fs::read_dir(root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(e)),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(io_err)? {
        if entry.file_name().to_string_lossy().starts_with('.') {
            continue;
        }
        if entry.file_type().await.is_ok_and(|t| t.is_dir()) {
            out.push(entry.path());
        }
    }
    Ok(out)
}
