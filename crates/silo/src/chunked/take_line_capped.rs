// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::find_crlf::find_crlf;
use crate::multipart::s3_err;
use bytes::BytesMut;
use pistack_types::ApiResult;

/// Cap line length; scan incrementally via `scan_from`.
pub(in crate::chunked) fn take_line_capped(
    buf: &mut BytesMut,
    scan_from: &mut usize,
    max: usize,
    oversize_msg: &str,
) -> ApiResult<Option<String>> {
    if let Some(pos) = find_crlf(buf, *scan_from) {
        *scan_from = 0;
        let line = buf.split_to(pos);
        let _ = buf.split_to(2);
        return Ok(Some(String::from_utf8_lossy(&line).into_owned()));
    }
    if buf.len() > max {
        return Err(s3_err("InvalidRequest", oversize_msg));
    }
    *scan_from = buf.len().saturating_sub(1);
    Ok(None)
}
