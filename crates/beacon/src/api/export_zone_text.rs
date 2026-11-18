// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Extension, Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
};
use pistack_types::{ApiResult, Caller};

use super::api_state::ApiState;
use super::resolve_zone::resolve_zone;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn export_zone_text(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Response> {
    let project = scope(&caller, &sq)?;
    let cur = visible(&caller, resolve_zone(&st.client, project.as_ref(), &id)?)?;
    let text = st.client.export(&cur.meta.id)?;
    Ok(([(header::CONTENT_TYPE, "text/dns; charset=utf-8")], text).into_response())
}
