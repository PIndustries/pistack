// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use pistack_types::{ApiResult, Caller};

use super::load_bucket::load_bucket;

/// Resolve a copy-source bucket for UploadPartCopy.
///
/// Foreign-project and nonexistent names both yield S3 `AccessDenied` so
/// bucket existence is not leaked. A visible bucket is returned for the
/// subsequent key lookup (`NoSuchKey` stays distinct).
pub(crate) fn load_copy_source_bucket(
    client: &Client,
    caller: &Caller,
    name_or_id: &str,
) -> ApiResult<Bucket> {
    match load_bucket(client, caller, name_or_id) {
        Ok(b) => Ok(b),
        Err(_) => Err(crate::multipart::s3_err("AccessDenied", "Access Denied")),
    }
}
