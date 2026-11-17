// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::Alarm;
use axum::{
    extract::{Extension, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller, ListQuery, Page};

use super::page::page;
use super::scope::scope;
use super::scope_query::ScopeQuery;

pub(crate) async fn list_alarms(
    State(client): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Alarm>>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(page(client.list_alarms(project.as_ref())?, &q)))
}
