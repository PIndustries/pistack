// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, State},
    http::StatusCode,
};
use pistack_types::{ApiResult, Caller};

use super::caller::caller;
use super::load_bucket::load_bucket;

pub(crate) async fn delete_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    // The client refuses while a generation or a delete marker is left, so a
    // key hidden from the listing still counts as holding the bucket open.
    c.delete_bucket(&bucket.meta.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
