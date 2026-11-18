// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension, Json,
};
use pistack_types::{ApiError, ApiResult, Caller};

use crate::client::Client;
use crate::types::*;

use super::caller::caller;
use super::editable::editable;
use super::member_body::MemberBody;

pub(crate) async fn add_member(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<MemberBody>,
) -> ApiResult<(StatusCode, Json<ImageMember>)> {
    let img = editable(&c, &caller(ext)?, &id)?;
    if body.project.kind != "proj" {
        return Err(ApiError::bad_request("project must be a proj-… id"));
    }
    Ok((
        StatusCode::CREATED,
        Json(c.add_member(&img.meta.id, body.project)?),
    ))
}
