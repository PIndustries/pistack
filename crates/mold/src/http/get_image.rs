// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, State},
    Extension, Json,
};
use pistack_types::{ApiResult, Caller};

use crate::client::Client;
use crate::types::*;

use super::caller::caller;

pub(crate) async fn get_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
) -> ApiResult<Json<Image>> {
    Ok(Json(c.get_visible(&caller(ext)?, &id)?))
}
