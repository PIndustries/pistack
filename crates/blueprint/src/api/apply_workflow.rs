// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Apply a workflow: run its stacks in order.

use super::list_project::list_project;
use super::visible::visible;
use crate::client::Client;
use crate::types::Workflow;
use axum::{
    extract::{Extension, Path, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

pub(super) async fn apply_workflow(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Workflow>> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_workflow(project.as_ref(), &id)?)?;
    Ok(Json(client.apply_workflow(&cur.meta.id)?))
}
