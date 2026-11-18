// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::chunked::StreamingAuth;
use crate::multipart::s3_err;
use axum::{extract::Request, http::Method};
use pistack_types::{ApiError, ApiResult};

use super::auth_ok::AuthOk;
use super::build_canonical_request::build_canonical_request;
use super::build_string_to_sign::build_string_to_sign;
use super::check_copy_source_signed_headers::check_copy_source_signed_headers;
use super::check_region::check_region;
use super::check_streaming_signed_headers::check_streaming_signed_headers;
use super::check_time_skew::check_time_skew;
use super::classify_streaming::classify_streaming;
use super::ct_eq_hex::ct_eq_hex;
use super::derive_signing_key::derive_signing_key;
use super::header_str::header_str;
use super::hmac_sha256_hex::hmac_sha256_hex;
use super::is_hex_sha256::is_hex_sha256;
use super::parse_auth::parse_auth;
use super::request_path::request_path;
use super::sig_v4_state::SigV4State;

/// `Ok(None)` = no SigV4 material (leave for bearer/`require_caller`).
pub(crate) fn authenticate(state: &SigV4State, req: &Request) -> ApiResult<Option<AuthOk>> {
    let parsed = match parse_auth(req)? {
        None => return Ok(None),
        Some(p) => p,
    };

    let streaming_kind = classify_streaming(&parsed.payload_hash, req.headers())?;
    if let Some(ref kind) = streaming_kind {
        if parsed.presigned {
            return Err(s3_err(
                "InvalidRequest",
                "presigned URLs cannot use STREAMING-* payloads",
            ));
        }
        if *req.method() != Method::PUT {
            return Err(s3_err(
                "InvalidRequest",
                "STREAMING-* payload is only valid on PUT (PutObject/UploadPart)",
            ));
        }
        check_streaming_signed_headers(&parsed, kind)?;
    }
    check_copy_source_signed_headers(&parsed, req.headers())?;

    check_region(&parsed.region, &state.region)?;
    check_time_skew(parsed.amz_date, parsed.expires_secs)?;

    let Some(keep) = state.keep.as_ref() else {
        // Same external shape as a bad key: do not advertise that keep is down.
        return Err(s3_err(
            "InvalidAccessKeyId",
            "The AWS Access Key Id you provided does not exist in our records.",
        ));
    };

    let credential = match keep.lookup_access_key(&parsed.access_key_id) {
        Ok(c) => c,
        Err(keep::KeepError::Unauthorized(_)) => {
            return Err(s3_err(
                "InvalidAccessKeyId",
                "The AWS Access Key Id you provided does not exist in our records.",
            ));
        }
        Err(e) => {
            tracing::error!(error = %e, "silo sigv4: keep lookup failed");
            return Err(ApiError::internal("access key lookup failed"));
        }
    };

    let canonical = build_canonical_request(
        req.method(),
        request_path(req),
        req.uri().query(),
        req.headers(),
        &parsed,
    )?;
    let string_to_sign = build_string_to_sign(&parsed, &canonical);
    let signing_key =
        derive_signing_key(&credential.secret, &parsed.date_stamp, &parsed.region, "s3");
    let expected = hmac_sha256_hex(&signing_key, string_to_sign.as_bytes());
    if !ct_eq_hex(&expected, &parsed.signature) {
        return Err(s3_err(
            "SignatureDoesNotMatch",
            "The request signature we calculated does not match the signature you provided.",
        ));
    }

    let expected_payload = if is_hex_sha256(&parsed.payload_hash) {
        Some(parsed.payload_hash.to_ascii_lowercase())
    } else {
        None
    };

    let streaming = if let Some(kind) = streaming_kind {
        let decoded = header_str(req.headers(), "x-amz-decoded-content-length")
            .ok_or_else(|| {
                s3_err(
                    "InvalidRequest",
                    "missing x-amz-decoded-content-length for aws-chunked upload",
                )
            })?
            .parse::<u64>()
            .map_err(|_| {
                s3_err(
                    "InvalidRequest",
                    "x-amz-decoded-content-length must be a number",
                )
            })?;
        let amz_date = parsed.amz_date.format("%Y%m%dT%H%M%S").to_string() + "Z";
        let scope = format!("{}/{}/s3/aws4_request", parsed.date_stamp, parsed.region);
        Some(StreamingAuth {
            kind,
            seed_signature: parsed.signature.clone(),
            signing_key: signing_key.clone(),
            amz_date,
            scope,
            decoded_content_length: decoded,
        })
    } else {
        None
    };

    Ok(Some(AuthOk {
        caller: credential.caller,
        expected_payload_sha256: expected_payload,
        streaming,
    }))
}
