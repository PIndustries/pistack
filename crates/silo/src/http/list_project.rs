// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Caller, Id};

pub(crate) fn list_project(caller: &Caller) -> ApiResult<Option<Id>> {
    if caller.is_admin() {
        Ok(caller.project.clone())
    } else {
        caller
            .project
            .clone()
            .map(Some)
            .ok_or_else(|| ApiError::forbidden("token has no project scope"))
    }
}
