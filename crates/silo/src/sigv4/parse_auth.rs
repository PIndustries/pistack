// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::{extract::Request, http::header};
use pistack_types::ApiResult;

use super::is_aws4_authorization::is_aws4_authorization;
use super::parse_authorization_header::parse_authorization_header;
use super::parse_presigned::parse_presigned;
use super::parsed_auth::ParsedAuth;
use super::query_has::query_has;

pub(crate) fn parse_auth(req: &Request) -> ApiResult<Option<ParsedAuth>> {
    let headers = req.headers();
    let auths: Vec<_> = headers.get_all(header::AUTHORIZATION).iter().collect();
    if auths.len() > 1 {
        return Err(s3_err("InvalidArgument", "multiple Authorization headers"));
    }
    if let Some(auth) = auths.first().and_then(|v| v.to_str().ok()) {
        if is_aws4_authorization(auth) {
            return Ok(Some(parse_authorization_header(auth, headers)?));
        }
        // `AWS4-HMAC-SHA256garbage` is not SigV4; leave for other auth.
    }
    let q = req.uri().query().unwrap_or("");
    if query_has(q, "X-Amz-Algorithm") {
        return Ok(Some(parse_presigned(q, headers)?));
    }
    Ok(None)
}
