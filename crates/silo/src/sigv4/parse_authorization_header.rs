// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::header_str::header_str;
use super::parse_amz_date::parse_amz_date;
use super::parsed_auth::ParsedAuth;
use super::split_credential::split_credential;

pub(crate) fn parse_authorization_header(auth: &str, headers: &HeaderMap) -> ApiResult<ParsedAuth> {
    let rest = auth
        .get(16..)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            s3_err(
                "AuthorizationHeaderMalformed",
                "Authorization header malformed",
            )
        })?;
    let mut credential = None;
    let mut signed_headers = None;
    let mut signature = None;
    for part in rest.split(',') {
        let part = part.trim();
        if let Some(v) = part.strip_prefix("Credential=") {
            credential = Some(v.trim());
        } else if let Some(v) = part.strip_prefix("SignedHeaders=") {
            signed_headers = Some(v.trim());
        } else if let Some(v) = part.strip_prefix("Signature=") {
            signature = Some(v.trim());
        }
    }
    let credential = credential.ok_or_else(|| {
        s3_err(
            "AuthorizationHeaderMalformed",
            "Authorization header missing Credential",
        )
    })?;
    let signed_headers = signed_headers.ok_or_else(|| {
        s3_err(
            "AuthorizationHeaderMalformed",
            "Authorization header missing SignedHeaders",
        )
    })?;
    let signature = signature.ok_or_else(|| {
        s3_err(
            "AuthorizationHeaderMalformed",
            "Authorization header missing Signature",
        )
    })?;

    let (access_key_id, date_stamp, region) = split_credential(credential)?;
    let amz_date_raw = header_str(headers, "x-amz-date")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing x-amz-date header"))?;
    let amz_date = parse_amz_date(amz_date_raw)?;
    if amz_date.format("%Y%m%d").to_string() != date_stamp {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential date does not match x-amz-date",
        ));
    }
    let payload_hash = header_str(headers, "x-amz-content-sha256")
        .ok_or_else(|| s3_err("InvalidRequest", "missing x-amz-content-sha256 header"))?
        .to_owned();

    Ok(ParsedAuth {
        access_key_id,
        date_stamp,
        region,
        amz_date,
        signed_headers: signed_headers
            .split(';')
            .map(|s| s.trim().to_ascii_lowercase())
            .filter(|s| !s.is_empty())
            .collect(),
        signature: signature.to_ascii_lowercase(),
        payload_hash,
        expires_secs: None,
        presigned: false,
    })
}
