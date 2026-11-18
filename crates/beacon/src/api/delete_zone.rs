// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use pistack_types::{ApiResult, Caller};

use super::api_state::ApiState;
use super::asked::asked;
use super::resolve_zone::resolve_zone;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn delete_zone(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, resolve_zone(&st.client, project.as_ref(), &id)?)?;
    st.client.delete_zone(&cur.meta.id)?;
    asked(&st, &cur.spec.name);
    Ok(StatusCode::NO_CONTENT)
}
