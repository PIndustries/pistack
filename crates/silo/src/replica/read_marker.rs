// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::path::Path;

use super::marker::Marker;
use super::marker_path::marker_path;
use super::read_marker_file::read_marker_file;

pub(crate) async fn read_marker(root: &Path, bucket: &Id, key: &str) -> Option<Marker> {
    read_marker_file(&marker_path(root, bucket, key)).await
}
