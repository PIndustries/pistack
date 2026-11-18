// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiResult;

use super::s3_err::s3_err;

pub(crate) fn parse_bool_text(raw: &str) -> ApiResult<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" => Ok(true),
        "false" => Ok(false),
        _ => Err(s3_err("MalformedXML", "Quiet must be true or false")),
    }
}
