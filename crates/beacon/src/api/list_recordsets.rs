// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Extension, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller, ListQuery, Page};

use super::api_state::ApiState;
use super::page::page;
use super::resolve_zone::resolve_zone;
use super::scope::scope;
use super::scope_query::ScopeQuery;

pub(crate) async fn list_recordsets(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Query(q): Query<ListQuery>,
) -> ApiResult<Json<Page<RecordSet>>> {
    let project = scope(&caller, &sq)?;
    let mut items = st.client.recordsets().list(project.as_ref())?;
    if let Some(z) = &sq.zone {
        let zone = resolve_zone(&st.client, project.as_ref(), z)?;
        items.retain(|r| r.spec.zone == zone.meta.id);
    }
    Ok(Json(page(items, &q)))
}
