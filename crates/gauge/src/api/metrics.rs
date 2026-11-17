// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, State},
    response::IntoResponse,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::text_response::text_response;

pub(crate) async fn metrics(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
) -> ApiResult<impl IntoResponse> {
    if !caller.is_admin() {
        return Err(ApiError::forbidden(
            "metrics of every host and project are for a cloud admin",
        ));
    }
    Ok(text_response(client.metrics_text()))
}
