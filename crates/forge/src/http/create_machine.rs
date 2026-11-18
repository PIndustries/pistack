// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{new_machine, MachineSpec};
use axum::{extract::State, response::Response};
use pistack_types::{ApiError, ApiResult};
use serde::Deserialize;
use std::collections::BTreeMap;

use super::app_state::AppState;
use super::body::Body;
use super::check_zone::check_zone;
use super::created::created;
use super::keypair_public_key::keypair_public_key;
use super::refuse_volumes::refuse_volumes;
use super::resolve_size::resolve_size;
use super::scope::Scope;
use super::validate_name::validate_name;

#[derive(Debug, Deserialize)]
pub struct CreateMachine {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    pub spec: MachineSpec,
}

pub(crate) async fn create_machine(
    State(s): State<AppState>,
    scope: Scope,
    Body(body): Body<CreateMachine>,
) -> ApiResult<Response> {
    validate_name(&body.name)?;
    if body.spec.image.trim().is_empty() {
        return Err(ApiError::bad_request("spec.image is required"));
    }
    if body.spec.size.trim().is_empty() {
        return Err(ApiError::bad_request("spec.size is required"));
    }
    refuse_volumes(&body.spec.volumes)?;
    check_zone(&body.spec)?;
    let size = resolve_size(&s, &body.spec.size)?;
    let mut spec = body.spec;
    // The pair is resolved to its key for the host, but the name stays on the
    // machine: it is what `pi machine show` prints and what update refuses to
    // change. The host is given keys, never the name.
    if let Some(name) = &spec.keypair {
        if !name.trim().is_empty() {
            let key = keypair_public_key(&s, &scope, name)?;
            if !spec.ssh_keys.iter().any(|k| k == &key) {
                spec.ssh_keys.push(key);
            }
        }
    }
    // Store the canonical size name so renames of the request form do not
    // change what the scheduler looks up.
    spec.size = size.meta.name.clone();
    spec.restarts = 0;
    let mut m = new_machine(body.name, scope.project.clone(), spec);
    m.meta.description = body.description;
    m.meta.labels = body.labels;
    let m = crate::usage::create(&s.store, m, &size.spec.resources)?;
    Ok(created(m))
}
