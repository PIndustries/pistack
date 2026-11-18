// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use axum::{
    extract::{Extension, Path, Query, State},
    Json,
};
use pistack_types::{ApiError, ApiResult, Caller};

use super::associate::Associate;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;
use crate::types::FloatingIp;

pub(crate) async fn associate_fip(
    State(c): State<Client>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    Json(body): Json<Associate>,
) -> ApiResult<Json<FloatingIp>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, c.floating_ips().resolve(project.as_ref(), &id)?)?;
    let fip = match (body.port, body.machine) {
        (Some(port), None) => c.associate_floating_ip(&cur.meta.id, Some(port))?,
        (None, Some(machine)) => c.associate_to_machine(&cur.meta.id, &machine)?,
        _ => {
            return Err(ApiError::bad_request(
                "associate needs exactly one of port or machine",
            ))
        }
    };
    Ok(Json(fip))
}
