// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::PathBuf;

use super::mtime::mtime;
use super::staged_upload::StagedUpload;
use super::upload_meta::UploadMeta;

pub(crate) async fn read_staged(dir: PathBuf, upload_id: String) -> StagedUpload {
    let meta_path = dir.join("meta.json");
    let meta = match tokio::fs::read(&meta_path).await {
        Ok(bytes) => match serde_json::from_slice::<UploadMeta>(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::debug!(path = %meta_path.display(), error = %e, "silo: unreadable upload meta");
                None
            }
        },
        Err(_) => None,
    };
    let initiated = match meta.as_ref().and_then(|m| m.initiated) {
        Some(t) => Some(t),
        None => match mtime(&meta_path).await {
            Some(t) => Some(t),
            None => mtime(&dir).await,
        },
    };
    StagedUpload {
        upload_id,
        dir,
        meta,
        initiated,
    }
}
