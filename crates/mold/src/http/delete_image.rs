// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    http::StatusCode,
    Extension,
};
use pistack_types::{ApiResult, Caller};

use crate::client::Client;

use super::caller::caller;
use super::editable::editable;

pub(crate) async fn delete_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let img = editable(&c, &caller(ext)?, &id)?;
    c.delete(&img.meta.id).await?;
    Ok(StatusCode::NO_CONTENT)
}
