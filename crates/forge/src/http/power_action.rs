// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Machine, MachineSpec, DELETE_ANNOTATION};
use axum::Json;
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::load_machine::load_machine;
use super::machines::machines;
use super::scope::Scope;

pub(crate) async fn power_action(
    s: &AppState,
    scope: &Scope,
    id: &str,
    f: impl Fn(&mut MachineSpec),
) -> ApiResult<Json<Machine>> {
    let m = load_machine(s, scope, id)?;
    if m.meta.annotations.contains_key(DELETE_ANNOTATION) {
        return Err(ApiError::conflict("machine is being deleted"));
    }
    let m = machines(s).modify(&m.meta.id, |x| {
        f(&mut x.spec);
        Ok(())
    })?;
    Ok(Json(m))
}
