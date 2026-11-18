// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::apex_of::apex_of;
use super::api_state::ApiState;
use super::apply_meta::apply_meta;
use super::asked::asked;
use super::if_match::if_match;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::update::Update;
use super::visible::visible;

pub(crate) async fn update_recordset(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Update<RecordSetSpec>>,
) -> ApiResult<Json<RecordSet>> {
    let project = scope(&caller, &sq)?;
    let cur = visible(
        &caller,
        st.client.recordsets().resolve(project.as_ref(), &id)?,
    )?;
    let rev = if_match(&headers)?;
    let r = st.client.update_recordset(&cur.meta.id, rev, |r| {
        apply_meta(&mut r.meta, &body);
        r.spec = body.spec.clone();
        Ok(())
    })?;
    asked(&st, &apex_of(&st, &r.spec.zone).await);
    Ok(Json(r))
}
