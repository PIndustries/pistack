// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension,
};
use pistack_types::{ApiError, ApiResult, Caller, Id};

use crate::client::Client;

use super::caller::caller;
use super::editable::editable;

pub(crate) async fn remove_member(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path((id, project)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let img = editable(&c, &caller(ext)?, &id)?;
    let project = Id::parse("proj", &project)
        .ok_or_else(|| ApiError::bad_request(format!("'{project}' is not a project id")))?;
    c.remove_member(&img.meta.id, &project)?;
    Ok(StatusCode::NO_CONTENT)
}
