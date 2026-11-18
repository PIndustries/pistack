// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Extension, Json,
};
use pistack_types::{ApiError, ApiResult, Caller};

use crate::client::{Client, UpdateImage};
use crate::types::*;

use super::caller::caller;
use super::editable::editable;
use super::if_match::if_match;

pub(crate) async fn update_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(req): Json<UpdateImage>,
) -> ApiResult<Json<Image>> {
    let caller = caller(ext)?;
    let img = editable(&c, &caller, &id)?;
    if req.visibility == Some(Visibility::Public) && !caller.is_admin() {
        return Err(ApiError::forbidden("only admins publish public images"));
    }
    Ok(Json(c.update(&img.meta.id, req, if_match(&headers)?)?))
}
