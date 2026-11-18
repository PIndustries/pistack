// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use pistack_types::{ApiError, ApiResult, Id, Spec};

use super::authorize_peer::authorize_peer;

/// Fetch an object's bytes because the server that took the put asked.
///
/// The record travels through the cluster log and may arrive a moment
/// after this request, so wait for it. Answers once this server holds
/// bytes that match the checksum.
pub(crate) async fn peer_pull(
    State(c): State<Client>,
    headers: HeaderMap,
    Path((object, sha256)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let required = std::env::var(pistack_rpc::REQUIRE_JOIN_ENV).ok();
    authorize_peer(c.store(), &headers, required.as_deref())?;
    let id = Id::parse(ObjectSpec::KIND, &object)
        .ok_or_else(|| ApiError::not_found("no such object"))?;
    let deadline = tokio::time::Instant::now() + crate::replica::RECORD_WAIT;
    let obj = loop {
        match c.store().collection::<Object>().get(&id) {
            Ok(obj) if obj.status.sha256 == sha256 => break obj,
            _ if tokio::time::Instant::now() >= deadline => {
                return Err(ApiError::unavailable(
                    "the record of that object has not reached this server",
                ));
            }
            _ => tokio::time::sleep(std::time::Duration::from_millis(25)).await,
        }
    };
    c.ensure_bytes(&obj).await?;
    Ok(StatusCode::NO_CONTENT)
}
