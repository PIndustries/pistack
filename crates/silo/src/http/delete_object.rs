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

pub(crate) async fn delete_object(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, key)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let bucket = load_bucket(&c, &caller(ext)?, &id)?;
    c.delete_object(&bucket.meta.id, &key).await?;
    Ok(StatusCode::NO_CONTENT)
}
