// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::http::load_bucket;
use axum::{http::StatusCode, Extension};
use pistack_types::{ApiResult, Caller};

use super::caller::caller;

pub(crate) async fn head_bucket_inner(
    c: Client,
    ext: Option<Extension<Caller>>,
    bucket_name: String,
) -> ApiResult<StatusCode> {
    let who = caller(ext)?;
    load_bucket(&c, &who, &bucket_name)?;
    Ok(StatusCode::OK)
}
