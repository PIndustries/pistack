// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Create a workflow.

use super::create_project::create_project;
use super::create_workflow_body::CreateWorkflow;
use super::created::created;
use super::new_meta::new_meta;
use crate::client::Client;
use crate::types::WORKFLOW_KIND;
use axum::extract::{Extension, State};
use axum::response::Response;
use axum::Json;
use pistack_types::{ApiResult, Caller};

pub(super) async fn create_workflow(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Json(body): Json<CreateWorkflow>,
) -> ApiResult<Response> {
    let project = create_project(&caller, &body.project)?;
    let meta = new_meta(
        WORKFLOW_KIND,
        &body.name,
        body.description,
        body.labels,
        body.annotations,
        project,
    )?;
    Ok(created(client.create_workflow(meta, body.spec)?))
}
