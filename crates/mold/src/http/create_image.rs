// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{extract::State, http::StatusCode, Extension, Json};
use pistack_types::{ApiError, ApiResult, Caller};

use crate::client::{Client, CreateImage};
use crate::types::*;

use super::caller::caller;

pub(crate) async fn create_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Json(mut req): Json<CreateImage>,
) -> ApiResult<(StatusCode, Json<Image>)> {
    let caller = caller(ext)?;
    match (&req.project, &caller.project) {
        (Some(p), Some(own)) if p != own && !caller.is_admin() => {
            return Err(ApiError::forbidden(
                "cannot create images in another project",
            ));
        }
        (None, own) => req.project = own.clone(),
        _ => {}
    }
    if req.spec.visibility == Visibility::Public && !caller.is_admin() {
        return Err(ApiError::forbidden("only admins publish public images"));
    }
    Ok((StatusCode::CREATED, Json(c.create(req)?)))
}
