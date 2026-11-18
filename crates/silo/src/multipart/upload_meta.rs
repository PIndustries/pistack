// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadMeta {
    pub bucket: String,
    pub key: String,
    pub content_type: String,
    /// When CreateMultipartUpload ran. Absent on uploads staged before this
    /// field existed; readers fall back to the `meta.json` mtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiated: Option<DateTime<Utc>>,
}
