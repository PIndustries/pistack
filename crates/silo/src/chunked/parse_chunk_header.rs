// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use pistack_types::ApiResult;

fn parse_hex_size(s: &str) -> ApiResult<usize> {
    let s = s.trim();
    if s.is_empty() || s.len() > 16 || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(s3_err("InvalidRequest", "invalid aws-chunked chunk size"));
    }
    usize::from_str_radix(s, 16)
        .map_err(|_| s3_err("InvalidRequest", "invalid aws-chunked chunk size"))
}

fn parse_chunk_sig(rest: &str, require_sig: bool) -> ApiResult<Option<String>> {
    let rest = rest.trim();
    if rest.is_empty() {
        if require_sig {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked chunk missing chunk-signature",
            ));
        }
        return Ok(None);
    }
    let (k, v) = rest
        .split_once('=')
        .ok_or_else(|| s3_err("InvalidRequest", "malformed chunk-signature extension"))?;
    if !k.eq_ignore_ascii_case("chunk-signature") {
        return Err(s3_err(
            "InvalidRequest",
            format!("unsupported chunk extension '{k}'"),
        ));
    }
    let v = v.trim().to_ascii_lowercase();
    if v.len() != 64 || !v.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(s3_err(
            "InvalidRequest",
            "chunk-signature must be 64 hex characters",
        ));
    }
    Ok(Some(v))
}

pub(in crate::chunked) fn parse_chunk_header(
    line: &str,
    require_sig: bool,
) -> ApiResult<(usize, Option<String>)> {
    let line = line.trim();
    if line.is_empty() {
        return Err(s3_err("InvalidRequest", "empty aws-chunked chunk header"));
    }
    if let Some((size_hex, rest)) = line.split_once(';') {
        let size = parse_hex_size(size_hex)?;
        let sig = parse_chunk_sig(rest.trim(), require_sig)?;
        Ok((size, sig))
    } else {
        let size = parse_hex_size(line)?;
        if require_sig {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked chunk missing chunk-signature",
            ));
        }
        Ok((size, None))
    }
}
