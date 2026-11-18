// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::http::{HeaderMap, Method};
use pistack_types::ApiResult;

use super::canonical_headers::canonical_headers;
use super::canonical_query::canonical_query;
use super::canonical_uri::canonical_uri;
use super::parsed_auth::ParsedAuth;

pub(crate) fn build_canonical_request(
    method: &Method,
    path: &str,
    query: Option<&str>,
    headers: &HeaderMap,
    auth: &ParsedAuth,
) -> ApiResult<String> {
    let mut out = String::new();
    out.push_str(method.as_str());
    out.push('\n');
    out.push_str(&canonical_uri(path));
    out.push('\n');
    out.push_str(&canonical_query(query, auth.presigned));
    out.push('\n');
    out.push_str(&canonical_headers(headers, &auth.signed_headers)?);
    out.push('\n');
    out.push_str(&auth.signed_headers.join(";"));
    out.push('\n');
    out.push_str(&auth.payload_hash);
    Ok(out)
}
