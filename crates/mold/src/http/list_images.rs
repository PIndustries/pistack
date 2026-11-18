// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Query, State},
    Extension, Json,
};
use pistack_types::{ApiResult, Caller, ListQuery, Page};

use crate::client::Client;
use crate::types::*;

use super::caller::caller;
use super::page::page;

pub(crate) async fn list_images(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Image>>> {
    Ok(Json(page(c.list_visible(&caller(ext)?)?, &q)))
}
