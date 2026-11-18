// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::UploadMeta;
use serde::{Deserialize, Serialize};

use super::staged_part::StagedPart;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagedUpload {
    pub bucket: String,
    pub upload_id: String,
    pub meta: UploadMeta,
    #[serde(default)]
    pub parts: Vec<StagedPart>,
}
