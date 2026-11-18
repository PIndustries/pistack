// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use pistack_types::ApiResult;

use super::bucket_id::bucket_id;
use super::peer_allowed::peer_allowed;

/// Copy an upload's `meta.json` here because the server that created it
/// asked.
pub(crate) async fn peer_upload_pull(
    State(c): State<Client>,
    headers: HeaderMap,
    Path((bucket, upload)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    peer_allowed(&c, &headers)?;
    c.pull_staged(&bucket_id(&bucket)?, &upload, crate::staging::Want::Meta)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}
