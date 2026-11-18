// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::header_str::header_str;
use super::max_presign_expires_secs::MAX_PRESIGN_EXPIRES_SECS;
use super::parse_amz_date::parse_amz_date;
use super::parse_query_map::parse_query_map;
use super::parsed_auth::ParsedAuth;
use super::split_credential::split_credential;
use super::unsigned_payload::UNSIGNED_PAYLOAD;

pub(crate) fn parse_presigned(query: &str, headers: &HeaderMap) -> ApiResult<ParsedAuth> {
    let params = parse_query_map(query);
    let algo = params
        .get("X-Amz-Algorithm")
        .map(String::as_str)
        .unwrap_or("");
    if algo != "AWS4-HMAC-SHA256" {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "X-Amz-Algorithm must be AWS4-HMAC-SHA256",
        ));
    }
    let credential = params
        .get("X-Amz-Credential")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Credential"))?;
    let (access_key_id, date_stamp, region) = split_credential(credential)?;
    let amz_date_raw = params
        .get("X-Amz-Date")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Date"))?;
    let amz_date = parse_amz_date(amz_date_raw)?;
    if amz_date.format("%Y%m%d").to_string() != date_stamp {
        return Err(s3_err(
            "AuthorizationHeaderMalformed",
            "Credential date does not match X-Amz-Date",
        ));
    }
    let expires: i64 = params
        .get("X-Amz-Expires")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Expires"))?
        .parse()
        .map_err(|_| s3_err("InvalidRequest", "X-Amz-Expires must be a number"))?;
    if expires <= 0 || expires > MAX_PRESIGN_EXPIRES_SECS {
        return Err(s3_err(
            "InvalidRequest",
            format!("X-Amz-Expires must be 1..={MAX_PRESIGN_EXPIRES_SECS}"),
        ));
    }
    let signed_headers = params
        .get("X-Amz-SignedHeaders")
        .ok_or_else(|| {
            s3_err(
                "AuthorizationHeaderMalformed",
                "missing X-Amz-SignedHeaders",
            )
        })?
        .split(';')
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>();
    let signature = params
        .get("X-Amz-Signature")
        .ok_or_else(|| s3_err("AuthorizationHeaderMalformed", "missing X-Amz-Signature"))?
        .to_ascii_lowercase();

    // Presigned GETs use UNSIGNED-PAYLOAD; a query X-Amz-Content-Sha256 may
    // override. Header x-amz-content-sha256 is unusual on presign but accepted
    // if signed.
    let payload_hash = params
        .get("X-Amz-Content-Sha256")
        .cloned()
        .or_else(|| header_str(headers, "x-amz-content-sha256").map(|s| s.to_owned()))
        .unwrap_or_else(|| UNSIGNED_PAYLOAD.to_owned());

    Ok(ParsedAuth {
        access_key_id,
        date_stamp,
        region,
        amz_date,
        signed_headers,
        signature,
        payload_hash,
        expires_secs: Some(expires),
        presigned: true,
    })
}
