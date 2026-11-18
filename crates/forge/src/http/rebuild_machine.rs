// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, DELETE_ANNOTATION, REBUILD_ANNOTATION};
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::{ApiError, ApiResult, Phase};

use super::app_state::AppState;
use super::body::Body;
use super::load_machine::load_machine;
use super::machines::machines;
use super::rebuild_body::RebuildBody;
use super::resolve_size::resolve_size;
use super::scope::Scope;

/// Replace the machine's root disk with a fresh copy of an image and start
/// it again. Everything the machine is stays: id, name, host, ports,
/// addresses, volumes and keys. The call is accepted for a machine that is
/// ready or stopped and returns as it enters the `updating` phase; see
/// [`crate::reconcile::Reconciler::rebuild`] for what the host does.
pub(crate) async fn rebuild_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<RebuildBody>,
) -> ApiResult<Json<Machine>> {
    let m = load_machine(&s, &scope, &id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Err(ApiError::conflict("machine is being deleted"));
    }
    if m.status.phase != Phase::Ready && m.status.phase != Phase::Pending {
        return Err(ApiError::conflict(format!(
            "a {} machine cannot be rebuilt; stop it or wait for it to finish",
            crate::maintain::phase_word(m.status.phase)
        )));
    }
    let mut m = m;
    if let Some(image) = body.image.map(|i| i.trim().to_owned()) {
        if image.is_empty() {
            return Err(ApiError::bad_request("image must not be empty"));
        }
        m.spec.image = image;
    }
    if let Some(why) =
        crate::agent_driver::boot_image_problem(&s.store, m.meta.project.as_ref(), &m.spec.image)
    {
        return Err(ApiError::unprocessable(why));
    }
    let size = resolve_size(&s, &m.spec.size)?;
    if let Some(why) = crate::agent_driver::boot_image_size_problem(
        &s.store,
        m.meta.project.as_ref(),
        &m.spec.image,
        size.spec.resources.disk_gib,
    ) {
        return Err(ApiError::unprocessable(why));
    }
    let image = m.spec.image.clone();
    let phase = match m.status.phase {
        // Nothing runs yet: the request rides along to the placement and
        // the reconciler rebuilds the machine once it has a host.
        Phase::Pending => Phase::Pending,
        // Placed: the reconciler turns `Updating` into the rebuild.
        _ => Phase::Updating,
    };
    let m = machines(&s).modify(&m.meta.id, |x| {
        x.spec = m.spec.clone();
        x.meta
            .annotations
            .insert(REBUILD_ANNOTATION.into(), image.clone());
        if x.status.phase != phase {
            x.status.phase = phase;
        }
        x.status.message = if phase == Phase::Pending {
            format!("will rebuild from image {image}")
        } else {
            format!("rebuilding from image {image}")
        };
        Ok(())
    })?;
    Ok(Json(m))
}
