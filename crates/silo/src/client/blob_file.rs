// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::path::{Path, PathBuf};

pub(crate) fn blob_file(root: &Path, bucket: &Id, sha: &str) -> Option<PathBuf> {
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(
        root.join(bucket.to_string())
            .join(".blobs")
            .join(sha.to_ascii_lowercase()),
    )
}
