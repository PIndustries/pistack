// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Create a stack.

use super::create_project::create_project;
use super::create_stack_body::CreateStack;
use super::created::created;
use super::new_meta::new_meta;
use crate::client::Client;
use axum::response::Response;
use axum::{
    extract::{Extension, State},
    Json,
};
use pistack_types::{kind, ApiResult, Caller};

pub(super) async fn create_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<CreateStack>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(
        kind::STACK,
        &body.name,
        body.description,
        body.labels,
        body.annotations,
        project,
    )?;
    Ok(created(client.create_stack(meta, body.spec)?))
}
