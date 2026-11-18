// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use axum::{extract::State, Json};
use pistack_types::{ApiResult, Page};

use super::app_state::AppState;
use super::list::List;
use super::machines::machines;
use super::paginate::paginate;
use super::scope::Scope;

pub(crate) async fn list_machines(
    State(s): State<AppState>,
    scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Machine>>> {
    let items = machines(&s).list(scope.filter())?;
    Ok(Json(paginate(items, &q)))
}
