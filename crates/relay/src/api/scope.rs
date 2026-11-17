// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Which project a request acts in, and which resources its caller may see.

use super::body::ScopeQuery;
use pistack_types::{ApiError, ApiResult, Caller, Id};

pub(super) fn scope(caller: &Caller, q: &ScopeQuery) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        if q.all == Some(true) {
            return Ok(None);
        }
        return Ok(q.project.clone().or_else(|| caller.project.clone()));
    }
    if q.project.is_some() && q.project != caller.project {
        return Err(ApiError::forbidden("project scope does not match token"));
    }
    caller
        .project
        .clone()
        .map(Some)
        .ok_or_else(|| ApiError::forbidden("token has no project scope"))
}

pub(super) fn create_project(caller: &Caller, body: &Option<Id>) -> ApiResult<Option<Id>> {
    match (caller.is_admin(), body, &caller.project) {
        (true, Some(p), _) => Ok(Some(p.clone())),
        (true, None, p) => Ok(p.clone()),
        (false, Some(p), Some(mine)) if p == mine => Ok(Some(p.clone())),
        (false, None, Some(mine)) => Ok(Some(mine.clone())),
        (false, _, _) => Err(ApiError::forbidden(
            "cannot create outside the token's project",
        )),
    }
}
