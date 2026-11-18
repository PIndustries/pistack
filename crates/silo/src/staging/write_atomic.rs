// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(crate) async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}
