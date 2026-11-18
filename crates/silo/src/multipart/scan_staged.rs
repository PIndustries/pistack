// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;
use std::path::Path;

use super::io_err::io_err;
use super::read_staged::read_staged;
use super::staged_upload::StagedUpload;
use super::validate_upload_id::validate_upload_id;

/// Every staging dir directly under `uploads_root`. A missing root is an
/// empty list; entries that are not directories or have an invalid upload
/// id are skipped.
pub(crate) async fn scan_staged(uploads_root: &Path) -> ApiResult<Vec<StagedUpload>> {
    let mut entries = match tokio::fs::read_dir(uploads_root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(e)),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(io_err)? {
        let Ok(upload_id) = entry.file_name().into_string() else {
            continue;
        };
        if validate_upload_id(&upload_id).is_err() {
            continue;
        }
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        out.push(read_staged(entry.path(), upload_id).await);
    }
    Ok(out)
}
