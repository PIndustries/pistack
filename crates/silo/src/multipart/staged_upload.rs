// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use std::path::PathBuf;

use super::upload_meta::UploadMeta;

/// A staging directory found on disk. `meta` is `None` when `meta.json` is
/// missing or unreadable (for example a create that crashed half way).
#[derive(Clone, Debug)]
pub(crate) struct StagedUpload {
    pub upload_id: String,
    pub dir: PathBuf,
    pub meta: Option<UploadMeta>,
    /// `meta.initiated`, else the `meta.json` mtime, else the dir mtime.
    pub initiated: Option<DateTime<Utc>>,
}
