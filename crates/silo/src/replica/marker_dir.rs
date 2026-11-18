// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use std::path::{Path, PathBuf};

pub(crate) fn marker_dir(root: &Path, bucket: &Id) -> PathBuf {
    root.join(bucket.to_string()).join(".have")
}
