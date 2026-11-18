// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

use super::hex::hex;
use super::marker_dir::marker_dir;

pub(crate) fn marker_path(root: &Path, bucket: &Id, key: &str) -> PathBuf {
    marker_dir(root, bucket).join(hex(Sha256::digest(key.as_bytes()).as_slice()))
}
