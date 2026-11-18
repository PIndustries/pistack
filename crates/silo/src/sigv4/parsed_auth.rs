// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};

#[derive(Debug)]
pub(crate) struct ParsedAuth {
    pub(crate) access_key_id: String,
    pub(crate) date_stamp: String,
    pub(crate) region: String,
    pub(crate) amz_date: DateTime<Utc>,
    pub(crate) signed_headers: Vec<String>,
    pub(crate) signature: String,
    pub(crate) payload_hash: String,
    /// Presigned only: `X-Amz-Expires` seconds from `amz_date`.
    pub(crate) expires_secs: Option<i64>,
    /// Presigned auth uses UNSIGNED-PAYLOAD in the canonical request and
    /// excludes `X-Amz-Signature` from the canonical query string.
    pub(crate) presigned: bool,
}
