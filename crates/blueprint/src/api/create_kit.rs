// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Create a kit.

use super::create_kit_body::CreateKit;
use super::create_project::create_project;
use super::created::created;
use super::new_meta::new_meta;
use crate::client::Client;
use crate::types::KIT_KIND;
use axum::extract::{Extension, State};
use axum::response::Response;
use axum::Json;
use pistack_types::{ApiResult, Caller};

pub(super) async fn create_kit(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<CreateKit>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(
        KIT_KIND,
        &body.name,
        body.description,
        body.labels,
        body.annotations,
        project,
    )?;
    Ok(created(client.create_kit(meta, body.spec)?))
}
