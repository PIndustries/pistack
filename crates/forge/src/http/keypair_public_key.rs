// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use super::app_state::AppState;
use super::load_keypair::load_keypair;
use super::scope::Scope;

pub(crate) fn keypair_public_key(
    s: &AppState,
    scope: &Scope,
    name_or_id: &str,
) -> ApiResult<String> {
    let kp = load_keypair(s, scope, name_or_id).map_err(|_| {
        ApiError::unprocessable(format!(
            "keypair '{name_or_id}' not found in this project; create it with \
             `pi keypair create {name_or_id}`"
        ))
    })?;
    Ok(kp.spec.public_key.clone())
}
