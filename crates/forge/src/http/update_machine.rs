// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, MachineSpec};
use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use pistack_types::{ApiError, ApiResult, Phase};
use serde::Deserialize;
use std::collections::BTreeMap;

use super::app_state::AppState;
use super::body::Body;
use super::check_zone::check_zone;
use super::if_match::if_match;
use super::keypair_same::keypair_same;
use super::load_machine::load_machine;
use super::machines::machines;
use super::refuse_volumes::refuse_volumes;
use super::resolve_size::resolve_size;
use super::scope::Scope;
use super::sizes::sizes;

#[derive(Debug, Deserialize, Default)]
pub struct UpdateMachine {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub spec: Option<MachineSpec>,
}

pub(crate) async fn update_machine(
    State(s): State<AppState>,
    scope: Scope,
    Path(id): Path<String>,
    headers: HeaderMap,
    Body(body): Body<UpdateMachine>,
) -> ApiResult<Json<Machine>> {
    let expected = if_match(&headers)?;
    let mut m = load_machine(&s, &scope, &id)?;
    if let Some(d) = body.description {
        m.meta.description = d;
    }
    if let Some(l) = body.labels {
        m.meta.labels = l;
    }
    if let Some(mut spec) = body.spec {
        // A key pair is a create-time name; an update carries the keys a
        // machine has, so asking to change the pair is refused rather than
        // quietly ignored. The name is compared to the stored name and to its
        // id, since `pi apply` sends back whatever a read returned and an
        // id is a legal way to name the pair.
        if let Some(want) = spec.keypair.as_deref() {
            let held = m.spec.keypair.as_deref();
            // An id is a legal way to name the pair, so name and id both
            // count as the same pair.
            let same = held.is_some_and(|h| keypair_same(&s, &scope, want, h));
            if !same {
                return Err(ApiError::unprocessable(
                    "spec.keypair is set when the machine is created and cannot change after it",
                ));
            }
        }
        spec.keypair = m.spec.keypair.clone();
        if m.status.phase != Phase::Pending
            && (spec.kind != m.spec.kind || spec.image != m.spec.image || spec.size != m.spec.size)
        {
            return Err(ApiError::unprocessable(
                "kind, image and size can only change while the machine is pending",
            ));
        }
        // A machine stored with volumes before they were refused keeps them.
        if spec.volumes != m.spec.volumes {
            refuse_volumes(&spec.volumes)?;
        }
        check_zone(&spec)?;
        let size = resolve_size(&s, &spec.size)?;
        spec.size = size.meta.name.clone();
        // Reboot requests are made through the action endpoint, never by PUT.
        spec.restarts = m.spec.restarts;
        if spec.size != m.spec.size {
            // The only call that changes a machine's size. A size that is
            // gone counted as nothing, so the new one is charged in full.
            let from = sizes(&s)
                .resolve(None, &m.spec.size)
                .map(|old| old.spec.resources)
                .unwrap_or_default();
            let project = m.meta.project.clone();
            m.spec = spec;
            let write = || Ok(machines(&s).update(m, expected)?);
            let m = crate::usage::resize(
                &s.store,
                project.as_ref(),
                &from,
                &size.spec.resources,
                write,
            )?;
            return Ok(Json(m));
        }
        m.spec = spec;
    }
    Ok(Json(machines(&s).update(m, expected)?))
}
