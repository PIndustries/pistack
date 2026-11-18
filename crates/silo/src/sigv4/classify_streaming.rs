// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::{
    StreamingKind, STREAMING_PAYLOAD, STREAMING_SIGNED_TRAILER, STREAMING_UNSIGNED_TRAILER,
};
use crate::multipart::s3_err;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::trailer_from_headers::trailer_from_headers;

pub(crate) fn classify_streaming(
    payload_hash: &str,
    headers: &HeaderMap,
) -> ApiResult<Option<StreamingKind>> {
    let p = payload_hash.trim();
    if p.eq_ignore_ascii_case(STREAMING_PAYLOAD) {
        return Ok(Some(StreamingKind::Signed));
    }
    if p.eq_ignore_ascii_case(STREAMING_UNSIGNED_TRAILER) {
        let checksum = trailer_from_headers(headers)?;
        return Ok(Some(StreamingKind::UnsignedTrailer { checksum }));
    }
    if p.eq_ignore_ascii_case(STREAMING_SIGNED_TRAILER) {
        let checksum = trailer_from_headers(headers)?;
        return Ok(Some(StreamingKind::SignedTrailer { checksum }));
    }
    if p.to_ascii_uppercase().starts_with("STREAMING-AWS4-ECDSA")
        || p.to_ascii_uppercase().contains("ECDSA-P256")
    {
        return Err(s3_err(
            "NotImplemented",
            "STREAMING-AWS4-ECDSA-P256-SHA256-* is not supported",
        ));
    }
    if p.to_ascii_uppercase().starts_with("STREAMING-") {
        return Err(s3_err(
            "NotImplemented",
            format!("streaming payload mode '{p}' is not supported"),
        ));
    }
    Ok(None)
}
