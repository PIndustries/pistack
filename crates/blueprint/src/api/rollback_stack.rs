// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Roll a stack back to an earlier revision and apply it.

use super::list_project::list_project;
use super::rollback_stack_body::RollbackStack;
use super::visible::visible;
use crate::client::Client;
use crate::types::Stack;
use axum::{
    body::Bytes,
    extract::{Extension, Path, State},
    Json,
};
use pistack_types::{ApiError, ApiResult, Caller};

pub(super) async fn rollback_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
    body: Bytes,
) -> ApiResult<Json<Stack>> {
    let body: RollbackStack = if body.iter().all(u8::is_ascii_whitespace) {
        RollbackStack::default()
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ApiError::bad_request(format!("invalid rollback body: {e}")))?
    };
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_stack(project.as_ref(), &id)?)?;
    Ok(Json(client.rollback_stack(&cur.meta.id, body.revision)?))
}
