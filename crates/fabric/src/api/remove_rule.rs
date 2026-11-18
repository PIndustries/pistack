// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;
use crate::types::SecurityGroup;

pub(crate) async fn remove_rule(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, rule)): Path<(String, uuid::Uuid)>,
) -> ApiResult<Json<SecurityGroup>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.security_groups().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.remove_rule(&cur.meta.id, rule)?))
}
