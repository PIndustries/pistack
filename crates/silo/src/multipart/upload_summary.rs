// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};

/// One in-progress upload, as returned by ListMultipartUploads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadSummary {
    pub key: String,
    pub upload_id: String,
    pub initiated: DateTime<Utc>,
}
