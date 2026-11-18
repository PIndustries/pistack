// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Keypair;
use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::keypairs::keypairs;
use super::scope::Scope;

pub(crate) fn load_keypair(s: &AppState, scope: &Scope, id: &str) -> ApiResult<Keypair> {
    let k = keypairs(s).resolve(scope.filter(), id)?;
    if !scope.can_see(k.meta.project.as_ref()) {
        return Err(ApiError::not_found(format!("keypair '{id}' not found")));
    }
    Ok(k)
}
