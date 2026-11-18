// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use pistack_types::{ApiResult, Caller, Page};

use crate::client::Client;
use crate::types::*;

use super::caller::caller;

pub(crate) async fn list_members(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Page<ImageMember>>> {
    let img = c.get_visible(&caller(ext)?, &id)?;
    let items = c.list_members(&img.meta.id)?;
    let total = items.len();
    Ok(Json(Page {
        items,
        next: None,
        total,
    }))
}
