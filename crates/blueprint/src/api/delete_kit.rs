// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Delete a kit.

use super::list_project::list_project;
use super::visible::visible;
use crate::client::Client;
use axum::extract::{Extension, Path, State};
use axum::http::StatusCode;
use pistack_types::{ApiResult, Caller};

pub(super) async fn delete_kit(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = list_project(&caller)?;
    let cur = visible(&caller, client.resolve_kit(project.as_ref(), &id)?)?;
    client.delete_kit(&cur.meta.id)?;
    Ok(StatusCode::NO_CONTENT)
}
