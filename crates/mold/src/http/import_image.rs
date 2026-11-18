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
use super::editable::editable;
use super::import_body::ImportBody;

pub(crate) async fn import_image(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    Json(body): Json<ImportBody>,
) -> ApiResult<Json<Image>> {
    let img = editable(&c, &caller(ext)?, &id)?;
    Ok(Json(c.import(&img.meta.id, &body.url).await?))
}
