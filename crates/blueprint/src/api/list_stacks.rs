// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! List stacks in the caller's project.

use super::list_project::list_project;
use super::page::page;
use crate::client::Client;
use crate::types::Stack;
use axum::{
    extract::{Extension, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller, ListQuery, Page};

pub(super) async fn list_stacks(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Stack>>> {
    let project = list_project(&caller)?;
    Ok(Json(page(client.list_stacks(project.as_ref())?, &q)))
}
