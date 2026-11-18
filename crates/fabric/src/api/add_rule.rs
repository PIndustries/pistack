// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, State},
    response::Response,
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::created::created;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn add_rule(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(rule): Json<Rule>,
) -> ApiResult<Response> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.security_groups().resolve(project.as_ref(), &id)?)?;
    Ok(created(c.add_rule(&cur.meta.id, rule)?))
}
