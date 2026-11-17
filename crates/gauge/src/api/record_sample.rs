// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::catalog::RESERVED_PREFIX;
use crate::client::Client;
use axum::{extract::State, response::Response, Json};
use pistack_types::{ApiError, ApiResult};

use super::created::created;
use super::sample_in::SampleIn;

pub(crate) async fn record_sample(
    State(client): State<Client>,
    Json(body): Json<SampleIn>,
) -> ApiResult<Response> {
    if body.metric.trim().starts_with(RESERVED_PREFIX) {
        return Err(ApiError::bad_request(format!(
            "metric names that start with {RESERVED_PREFIX} are recorded by PiStack and cannot be posted"
        )));
    }
    Ok(created(client.record_sample(body.metric, body.value)?))
}
