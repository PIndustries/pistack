// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller, ListQuery, Page};

use super::caller::caller;
use super::list_project::list_project;
use super::page::page;

pub(crate) async fn list_buckets(
    State(c): State<Client>,
    ext: Option<Extension<Caller>>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<Bucket>>> {
    let project = list_project(&caller(ext)?)?;
    Ok(Json(page(c.list_buckets(project.as_ref())?, &q)))
}
