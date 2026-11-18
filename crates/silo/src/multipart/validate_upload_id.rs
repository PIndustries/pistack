// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::s3_err::s3_err;

pub(crate) fn validate_upload_id(id: &str) -> ApiResult<()> {
    if id.is_empty() || id.len() > 128 || id.contains('/') || id.contains('\\') || id.contains("..")
    {
        return Err(s3_err("NoSuchUpload", "invalid upload id"));
    }
    Ok(())
}
