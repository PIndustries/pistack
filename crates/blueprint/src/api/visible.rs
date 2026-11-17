// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Showing an item only when its project is one the caller may see.

use pistack_types::{ApiError, ApiResult, Caller, Kind};

pub(super) fn visible<K: Kind>(caller: &Caller, item: K) -> ApiResult<K> {
    let project = item.meta().project.clone();
    if caller.is_admin() || project.is_none() || project == caller.project {
        Ok(item)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            item.meta().id
        )))
    }
}
