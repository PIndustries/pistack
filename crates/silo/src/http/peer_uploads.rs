// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{extract::State, http::HeaderMap, Json};
use pistack_types::ApiResult;

use super::peer_allowed::peer_allowed;

/// Every multipart upload this server stages, for another server to copy.
pub(crate) async fn peer_uploads(
    State(c): State<Client>,
    headers: HeaderMap,
) -> ApiResult<Json<crate::staging::Staged>> {
    peer_allowed(&c, &headers)?;
    Ok(Json(c.staged(None).await?))
}
