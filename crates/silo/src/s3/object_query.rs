// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct ObjectQuery {
    /// Presence of `uploads` (even empty) means CreateMultipartUpload on POST.
    pub uploads: Option<String>,
    #[serde(rename = "uploadId")]
    pub upload_id: Option<String>,
    #[serde(rename = "partNumber")]
    pub part_number: Option<String>,
    /// One generation of the key, as its decimal record version.
    #[serde(rename = "versionId")]
    pub version_id: Option<String>,
}
