// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The project scope a list or get request runs under.

use pistack_types::{ApiError, ApiResult, Caller, Id};

pub(super) fn list_project(caller: &Caller) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        return Ok(caller.project.clone());
    }
    caller
        .project
        .clone()
        .map(Some)
        .ok_or_else(|| ApiError::forbidden("token has no project scope"))
}
