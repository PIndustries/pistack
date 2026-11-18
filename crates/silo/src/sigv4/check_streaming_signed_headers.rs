// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::StreamingKind;
use crate::multipart::s3_err;
use pistack_types::ApiResult;

use super::parsed_auth::ParsedAuth;

pub(crate) fn check_streaming_signed_headers(
    parsed: &ParsedAuth,
    kind: &StreamingKind,
) -> ApiResult<()> {
    let has = |name: &str| {
        parsed
            .signed_headers
            .iter()
            .any(|h| h.eq_ignore_ascii_case(name))
    };
    if !has("x-amz-decoded-content-length") {
        return Err(s3_err(
            "AccessDenied",
            "x-amz-decoded-content-length must be in SignedHeaders for streaming uploads",
        ));
    }
    match kind {
        StreamingKind::UnsignedTrailer { .. } | StreamingKind::SignedTrailer { .. } => {
            if !has("x-amz-trailer") {
                return Err(s3_err(
                    "AccessDenied",
                    "x-amz-trailer must be in SignedHeaders for streaming trailer uploads",
                ));
            }
        }
        StreamingKind::Signed => {}
    }
    Ok(())
}
