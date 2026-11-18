// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::path::Path;

use super::marker_path::marker_path;

pub(crate) async fn remove_marker(root: &Path, bucket: &Id, key: &str) {
    let _ = tokio::fs::remove_file(marker_path(root, bucket, key)).await;
}
