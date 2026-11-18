// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use axum::{
    extract::{Extension, Path, Query, State},
    http::StatusCode,
};
use pistack_types::{ApiResult, Caller};

use super::apex_of::apex_of;
use super::api_state::ApiState;
use super::asked::asked;
use super::scope::scope;
use super::scope_query::ScopeQuery;
use super::visible::visible;

pub(crate) async fn delete_recordset(
    State(st): State<ApiState>,
    Extension(caller): Extension<Caller>,
    Query(sq): Query<ScopeQuery>,
    Path(id): Path<String>,
) -> ApiResult<StatusCode> {
    let project = scope(&caller, &sq)?;
    let cur = visible(
        &caller,
        st.client.recordsets().resolve(project.as_ref(), &id)?,
    )?;
    st.client.delete_recordset(&cur.meta.id)?;
    asked(&st, &apex_of(&st, &cur.spec.zone).await);
    Ok(StatusCode::NO_CONTENT)
}
