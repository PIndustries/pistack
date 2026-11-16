// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::checksum_state::ct_eq_hex::ct_eq_hex;
use crate::chunked::checksum_state::trailer_signature::trailer_signature;
use crate::chunked::consts::MAX_TRAILER_BYTES;
use crate::chunked::consts::MAX_TRAILER_LINES;
use crate::chunked::decoded_stream::decoded_stream::DecodedStream;
use crate::chunked::find_double_crlf::find_double_crlf;
use crate::chunked::split_header_line::split_header_line;
use crate::chunked::streaming_auth::streaming_kind::StreamingKind;
use crate::multipart::s3_err;
use pistack_types::ApiResult;

/// Wait until a full trailer (`…\r\n\r\n`) is buffered, then parse it.
/// Returns `Ok(true)` when consumed; `Ok(false)` needs more bytes.
pub(in crate::chunked) fn finish_trailer(this: &mut DecodedStream) -> ApiResult<bool> {
    let checksum_algo = this
        .auth
        .trailer_checksum()
        .ok_or_else(|| s3_err("InvalidRequest", "internal: trailer phase without checksum"))?;

    if this.buf.len() > MAX_TRAILER_BYTES {
        return Err(s3_err(
            "InvalidRequest",
            "aws-chunked trailing headers exceed size limit",
        ));
    }

    let Some(end) = find_double_crlf(&this.buf, this.scan_from) else {
        // No terminator yet — advance scan cursor (overlap of 3 for `\r\n\r`).
        this.scan_from = this.buf.len().saturating_sub(3);
        if this.buf.len() > MAX_TRAILER_BYTES {
            return Err(s3_err(
                "InvalidRequest",
                "aws-chunked trailing headers exceed size limit",
            ));
        }
        return Ok(false);
    };

    let section = this.buf.split_to(end);
    this.scan_from = 0;
    // `section` ends with `\r\n\r\n`.
    let text = String::from_utf8_lossy(&section[..section.len().saturating_sub(4)]);
    let mut lines: Vec<&str> = text.split("\r\n").collect();
    // split may leave a trailing empty from final content; drop empties at end only
    while lines.last().is_some_and(|l| l.is_empty()) {
        lines.pop();
    }
    if lines.len() > MAX_TRAILER_LINES {
        return Err(s3_err(
            "InvalidRequest",
            "aws-chunked trailing headers exceed line limit",
        ));
    }

    let want_name = checksum_algo.header_name();
    let mut got_value: Option<String> = None;
    let mut trailer_sig: Option<String> = None;
    for line in &lines {
        if line.is_empty() {
            continue;
        }
        let (k, v) = split_header_line(line)?;
        let k = k.to_ascii_lowercase();
        if k == want_name {
            if got_value.is_some() {
                return Err(s3_err(
                    "InvalidRequest",
                    format!("duplicate trailing checksum header {want_name}"),
                ));
            }
            got_value = Some(v.to_owned());
        } else if k == "x-amz-trailer-signature" {
            if trailer_sig.is_some() {
                return Err(s3_err(
                    "InvalidRequest",
                    "duplicate x-amz-trailer-signature trailing header",
                ));
            }
            trailer_sig = Some(v.to_ascii_lowercase());
        } else {
            return Err(s3_err(
                "InvalidRequest",
                format!("unexpected trailing header '{k}'"),
            ));
        }
    }
    let got_value = got_value.ok_or_else(|| {
        s3_err(
            "InvalidRequest",
            format!("missing trailing checksum header {want_name}"),
        )
    })?;

    if matches!(this.auth.kind, StreamingKind::SignedTrailer { .. }) {
        let declared = trailer_sig.ok_or_else(|| {
            s3_err(
                "InvalidRequest",
                "missing x-amz-trailer-signature trailing header",
            )
        })?;
        let header_line = format!("{want_name}:{got_value}\n");
        let expected = trailer_signature(
            &this.auth.signing_key,
            &this.auth.amz_date,
            &this.auth.scope,
            &this.prev_sig,
            &header_line,
        );
        if !ct_eq_hex(&expected, &declared) {
            return Err(s3_err(
                "SignatureDoesNotMatch",
                "The request signature we calculated does not match the signature you provided.",
            ));
        }
    } else if trailer_sig.is_some() {
        return Err(s3_err(
            "InvalidRequest",
            "x-amz-trailer-signature not expected for STREAMING-UNSIGNED-PAYLOAD-TRAILER",
        ));
    }

    let computed = this
        .checksum
        .take()
        .ok_or_else(|| s3_err("InternalError", "checksum state missing"))?
        .finalize_b64();
    if computed != got_value {
        return Err(s3_err(
            "XAmzContentChecksumMismatch",
            "The trailing checksum you specified did not match what the server calculated.",
        ));
    }
    Ok(true)
}
