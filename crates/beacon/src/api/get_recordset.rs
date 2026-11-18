// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Extension, Path, Query, State},
    Json,
};
use pistack_types::{ApiResult, Caller};

use super::api_state::ApiState;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn get_recordset(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<Json<RecordSet>> {
    let project = scope(&caller, &sq)?;
    Ok(Json(visible(
        &caller,
        st.client.recordsets().resolve(project.as_ref(), &id)?,
    )?))
}
