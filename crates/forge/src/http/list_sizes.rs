// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Size;
use axum::{extract::State, Json};
use pistack_types::{ApiResult, Page};

use super::app_state::AppState;
use super::list::List;
use super::paginate::paginate;
use super::scope::Scope;
use super::sizes::sizes;

pub(crate) async fn list_sizes(
    State(s): State<AppState>,
    _scope: Scope,
    List(q): List,
) -> ApiResult<Json<Page<Size>>> {
    Ok(Json(paginate(sizes(&s).list(None)?, &q)))
}
