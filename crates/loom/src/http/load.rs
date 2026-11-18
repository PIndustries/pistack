// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use crate::http::api::Api;
use crate::http::app_state::AppState;
use crate::http::col::col;
use crate::http::res::Res;
use crate::http::scope::Scope;

pub(crate) fn load<A: Api>(s: &AppState, scope: &Scope, id: &str) -> ApiResult<Res<A>> {
    let r = col::<A>(s).resolve(scope.filter(), id)?;
    if !scope.can_see(r.meta.project.as_ref()) {
        return Err(ApiError::not_found(format!("{} '{id}' not found", A::KIND)));
    }
    Ok(r)
}
