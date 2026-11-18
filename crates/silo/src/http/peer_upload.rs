// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use pistack_types::ApiResult;

use super::bucket_id::bucket_id;
use super::peer_allowed::peer_allowed;

pub(crate) async fn peer_upload(
    State(c): State<Client>,
    headers: HeaderMap,
    Path((bucket, upload)): Path<(String, String)>,
) -> ApiResult<Json<crate::staging::Staged>> {
    peer_allowed(&c, &headers)?;
    let bucket = bucket_id(&bucket)?;
    Ok(Json(c.staged(Some((&bucket, &upload))).await?))
}
