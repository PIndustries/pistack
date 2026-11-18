// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::path::Path;

use super::marker_path::marker_path;

/// Record that the file at `key` holds the bytes with checksum `sha256`.
pub(crate) async fn write_marker(
    root: &Path,
    bucket: &Id,
    key: &str,
    sha256: &str,
) -> std::io::Result<()> {
    let path = marker_path(root, bucket, key);
    if let Some(parent) = path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let tmp = path.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&tmp, format!("{sha256}\n{key}")).await?;
    tokio::fs::rename(&tmp, &path).await
}
