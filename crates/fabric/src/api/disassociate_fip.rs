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
use crate::types::FloatingIp;

pub(crate) async fn disassociate_fip(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<FloatingIp>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.floating_ips().resolve(project.as_ref(), &id)?)?;
    Ok(Json(c.associate_floating_ip(&cur.meta.id, None)?))
}
