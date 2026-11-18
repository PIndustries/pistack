// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Path, Request, State},
    Extension, Json,
};
use futures::TryStreamExt;
use pistack_types::{ApiResult, Caller};
use tokio_util::io::StreamReader;

use crate::client::Client;
use crate::types::*;

use super::caller::caller;
use super::editable::editable;

pub(crate) async fn upload(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Path(id): Path<String>,
    req: Request,
) -> ApiResult<Json<Image>> {
    let img = editable(&c, &caller(ext)?, &id)?;
    let stream = req
        .into_body()
        .into_data_stream()
        .map_err(std::io::Error::other);
    let reader = StreamReader::new(stream);
    Ok(Json(c.upload(&img.meta.id, Box::pin(reader)).await?))
}
