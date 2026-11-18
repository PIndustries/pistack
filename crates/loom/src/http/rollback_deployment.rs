// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::controllers::deployment::{owned_sets, revision_of};
use crate::selector::template_hash;
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
use crate::http::rollback_body::RollbackBody;
use crate::http::scope::Scope;
use crate::types::DeploymentSpec;

pub(crate) async fn rollback_deployment(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    body: Result<Body<RollbackBody>, ApiError>,
) -> ApiResult<Json<Deployment>> {
    // An empty body means "the previous revision".
    let body = body.map(|Body(b)| b).unwrap_or_default();
    let d = load::<DeploymentSpec>(&s, &scope, &id)?;
    if delete_requested(&d) {
        return Err(ApiError::conflict("deployment is being deleted"));
    }
    let sets = owned_sets(
        &s.store
            .collection::<ReplicaSet>()
            .list(d.meta.project.as_ref())?,
        &d,
    );
    let current_hash = template_hash(&d.spec.template);
    let target = match body.revision {
        Some(rev) => sets
            .iter()
            .find(|rs| revision_of(rs) == rev)
            .ok_or_else(|| ApiError::not_found(format!("revision {rev} not found")))?,
        None => sets
            .iter()
            .rev()
            .find(|rs| rs.meta.labels.get(POD_TEMPLATE_HASH) != Some(&current_hash))
            .ok_or_else(|| ApiError::unprocessable("no previous revision to roll back to"))?,
    };
    if target.meta.labels.get(POD_TEMPLATE_HASH) == Some(&current_hash) {
        return Err(ApiError::unprocessable(
            "that revision is already the current template",
        ));
    }
    let mut template = target.spec.template.clone();
    template.labels.remove(POD_TEMPLATE_HASH);
    let d = col::<DeploymentSpec>(&s).modify(&d.meta.id, |x| {
        x.spec.template = template.clone();
        Ok(())
    })?;
    Ok(Json(d))
}
