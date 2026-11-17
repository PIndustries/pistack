// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Caller, Kind};

pub(crate) fn visible<K: Kind>(caller: &Caller, resource: K) -> ApiResult<K> {
    let project = &resource.meta().project;
    if caller.is_admin() || project.is_none() || *project == caller.project {
        Ok(resource)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            resource.meta().id
        )))
    }
}
