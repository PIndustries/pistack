// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Debug, Deserialize, Default)]
pub struct ListQuery {
    #[serde(rename = "list-type")]
    pub list_type: Option<String>,
    pub prefix: Option<String>,
    #[serde(rename = "continuation-token")]
    pub continuation_token: Option<String>,
    #[serde(rename = "max-keys")]
    pub max_keys: Option<String>,
    #[serde(rename = "start-after")]
    pub start_after: Option<String>,
    /// Presence of `uploads` (even empty) means ListMultipartUploads.
    pub uploads: Option<String>,
    /// Presence of `versions` (even empty) means ListObjectVersions.
    pub versions: Option<String>,
    /// Presence of `versioning` (even empty) means the bucket's versioning
    /// configuration.
    pub versioning: Option<String>,
    #[serde(rename = "key-marker")]
    pub key_marker: Option<String>,
    #[serde(rename = "upload-id-marker")]
    pub upload_id_marker: Option<String>,
    #[serde(rename = "version-id-marker")]
    pub version_id_marker: Option<String>,
    #[serde(rename = "max-uploads")]
    pub max_uploads: Option<String>,
    pub delimiter: Option<String>,
    #[serde(rename = "encoding-type")]
    pub encoding_type: Option<String>,
}
