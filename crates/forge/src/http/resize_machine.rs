// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, DELETE_ANNOTATION};
use axum::{
    extract::{Path, State},
    Json,
};
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::body::Body;
use super::hosts::hosts;
use super::load_machine::load_machine;
use super::machines::machines;
use super::resize_body::ResizeBody;
use super::resolve_size::resolve_size;
use super::resources_of::resources_of;
use super::room::room;
use super::scope::Scope;

/// Change a stopped machine's size. The machine stays on its host: the host
/// must have room for what the new size adds. A larger root disk grows on
/// the next start; a smaller one than the disk already has is refused.
pub(crate) async fn resize_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    Body(body): Body<ResizeBody>,
) -> ApiResult<Json<Machine>> {
    let m = load_machine(&s, &scope, &id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Err(ApiError::conflict("machine is being deleted"));
    }
    if m.spec.power != crate::types::DesiredPower::Off
        || m.status.power != crate::types::PowerState::Off
    {
        return Err(ApiError::conflict(
            "the machine must be powered off; stop it first with `pi machine stop`",
        ));
    }
    if body.size.trim().is_empty() {
        return Err(ApiError::bad_request("size is required"));
    }
    let to = resolve_size(&s, &body.size)?;
    // The spec may hold the size's name or its id. Either way, the same size
    // is nothing to do.
    if m.spec.size == to.meta.name || m.spec.size == to.meta.id.to_string() {
        return Ok(Json(m));
    }
    let from = resources_of(&s, &m.spec.size);
    let to_res = to.spec.resources;
    if to_res.disk_gib < from.disk_gib {
        return Err(ApiError::unprocessable(format!(
            "size '{}' has a {} GiB root disk, smaller than the {} GiB disk the machine has",
            to.meta.name, to_res.disk_gib, from.disk_gib
        )));
    }
    if let Some(host) = m.status.host.as_ref() {
        let h = hosts(&s).get(host)?;
        let more = |to: u64, from: u64| to.saturating_sub(from);
        let need = pistack_types::Resources {
            vcpus: to_res.vcpus.saturating_sub(from.vcpus),
            memory_mib: more(to_res.memory_mib, from.memory_mib),
            disk_gib: more(to_res.disk_gib, from.disk_gib),
            gpus: to_res.gpus.saturating_sub(from.gpus),
        };
        if !need.fits_in(&h.status.free) {
            return Err(ApiError::conflict(format!(
                "host '{}' has no room for the resize: {} free, {} more needed; the machine is not moved to another host",
                h.meta.name,
                room(&h.status.free),
                room(&need)
            )));
        }
    }
    let expected = m.meta.revision;
    let project = m.meta.project.clone();
    let mut updated = m;
    updated.spec.size = to.meta.name.clone();
    let write = || {
        machines(&s)
            .update(updated.clone(), Some(expected))
            .map_err(Into::into)
    };
    let m = crate::usage::resize(&s.store, project.as_ref(), &from, &to_res, write)?;
    Ok(Json(m))
}
