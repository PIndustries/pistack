// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Machine;
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::machines::machines;
use super::scope::Scope;

pub(crate) fn load_machine(s: &AppState, scope: &Scope, id: &str) -> ApiResult<Machine> {
    let m = machines(s).resolve(scope.filter(), id)?;
    if !scope.can_see(m.meta.project.as_ref()) {
        return Err(ApiError::not_found(format!("mach '{id}' not found")));
    }
    Ok(m)
}
