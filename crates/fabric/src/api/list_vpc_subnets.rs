// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller, ListQuery, Page};

use super::page::page;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;
use crate::types::Subnet;

pub(crate) async fn list_vpc_subnets(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<Page<Subnet>>> {
    let project = scope(&caller, &sq)?;
    let vpc = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    let items = c.subnets_of_vpc(&vpc.meta.id)?;
    Ok(Json(page(items, &q)))
}
