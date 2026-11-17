// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Get one stack by name or id.

use super::list_project::list_project;
use super::visible::visible;
use crate::client::Client;
use crate::types::Stack;
use axum::{
    extract::{Extension, Path, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

pub(super) async fn get_stack(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<Json<Stack>> {
    let project = list_project(&caller)?;
    Ok(Json(visible(
        &caller,
        client.resolve_stack(project.as_ref(), &id)?,
    )?))
}
