// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    body::Body,
    extract::{Path, State},
    http::{header, HeaderMap, HeaderValue},
    response::{IntoResponse, Response},
};
use pistack_types::{ApiError, ApiResult, Id, Spec};
use tokio_util::io::ReaderStream;

use super::authorize_peer::authorize_peer;

/// Serve bytes to another control-plane server.
///
/// Same rule as `/cluster`: the caller must be a member, proved with the
/// peer key, unless `PISTACK_REQUIRE_JOIN=0` has opened the cluster routes.
pub(crate) async fn peer_blob(
    State(c): State<Client>,
    headers: HeaderMap,
    Path((bucket, sha256)): Path<(String, String)>,
) -> ApiResult<Response> {
    let required = std::env::var(pistack_rpc::REQUIRE_JOIN_ENV).ok();
    authorize_peer(c.store(), &headers, required.as_deref())?;
    let bad = || ApiError::not_found("no such bytes on this server");
    let bucket = Id::parse(BucketSpec::KIND, &bucket).ok_or_else(bad)?;
    if sha256.len() != 64 || !sha256.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(bad());
    }
    let (file, size) = c.open_blob(&bucket, &sha256.to_ascii_lowercase()).await?;
    let mut resp = Body::from_stream(ReaderStream::with_capacity(file, 256 * 1024)).into_response();
    let headers = resp.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    if let Ok(len) = HeaderValue::from_str(&size.to_string()) {
        headers.insert(header::CONTENT_LENGTH, len);
    }
    Ok(resp)
}
