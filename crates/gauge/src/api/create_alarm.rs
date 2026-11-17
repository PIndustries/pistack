// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::AlarmSpec;
use axum::{
    extract::{Extension, State},
    response::Response,
    Json,
};
use pistack_types::{kind, ApiResult, Caller};

use super::create_body::Create;
use super::create_project::create_project;
use super::created::created;
use super::new_meta::new_meta;

pub(crate) async fn create_alarm(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<Create<AlarmSpec>>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(kind::ALARM, &body, project)?;
    Ok(created(client.create_alarm(meta, body.spec)?))
}
