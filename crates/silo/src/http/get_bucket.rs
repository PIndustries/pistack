// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Path, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::caller::caller;
use super::load_bucket::load_bucket;

pub(crate) async fn get_bucket(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Bucket>> {
    Ok(Json(load_bucket(&c, &caller(ext)?, &id)?))
}
