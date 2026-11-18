// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::s3_err;
use axum::{extract::Request, http::header};
use pistack_types::ApiResult;

use super::is_aws4_authorization::is_aws4_authorization;
use super::query_has::query_has;

/// `true` when the request carries SigV4 header or presigned query auth.
/// Errors on multiple `Authorization` headers.
pub(crate) fn detect_sigv4_material(req: &Request) -> ApiResult<bool> {
    let auths: Vec<_> = req
        .headers()
        .get_all(header::AUTHORIZATION)
        .iter()
        .collect();
    if auths.len() > 1 {
        return Err(s3_err("InvalidArgument", "multiple Authorization headers"));
    }
    if let Some(raw) = auths.first().and_then(|v| v.to_str().ok()) {
        if is_aws4_authorization(raw) {
            return Ok(true);
        }
    }
    let q = req.uri().query().unwrap_or("");
    Ok(query_has(q, "X-Amz-Algorithm")
        || query_has(q, "X-Amz-Credential")
        || query_has(q, "X-Amz-Signature"))
}
