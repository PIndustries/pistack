// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

pub(in crate::chunked) fn split_header_line(line: &str) -> ApiResult<(&str, &str)> {
    let (k, v) = line
        .split_once(':')
        .ok_or_else(|| s3_err("InvalidRequest", "malformed trailing header"))?;
    Ok((k.trim(), v.trim()))
}
