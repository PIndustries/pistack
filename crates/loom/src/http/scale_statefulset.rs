// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::{ApiError, ApiResult};

use crate::http::app_state::AppState;
use crate::http::body::Body;
use crate::http::col::col;
use crate::http::load::load;
use crate::http::scale_body::ScaleBody;
use crate::http::scope::Scope;
use crate::types::StatefulSetSpec;

pub(crate) async fn scale_statefulset(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<ScaleBody>,
) -> ApiResult<Json<StatefulSet>> {
    let sts = load::<StatefulSetSpec>(&s, &scope, &id)?;
    if delete_requested(&sts) {
        return Err(ApiError::conflict("statefulset is being deleted"));
    }
    let sts = col::<StatefulSetSpec>(&s).modify(&sts.meta.id, |x| {
        x.spec.replicas = body.replicas;
        Ok(())
    })?;
    Ok(Json(sts))
}
