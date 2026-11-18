// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn delete_vpc_subnet(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path((id, subnet)): Path<(String, String)>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let vpc = visible(&caller, c.vpcs().resolve(project.as_ref(), &id)?)?;
    let sub = visible(&caller, c.subnets().resolve(project.as_ref(), &subnet)?)?;
    let net = c.networks().get(&sub.spec.network)?;
    if net.spec.vpc.as_ref() != Some(&vpc.meta.id) {
        return Err(ApiError::not_found(format!(
            "subnet {subnet} is not in vpc {}",
            vpc.meta.id
        )));
    }
    c.delete_subnet(&sub.meta.id)?;
    let _ = c.delete_network(&net.meta.id);
    Ok(StatusCode::NO_CONTENT)
}
