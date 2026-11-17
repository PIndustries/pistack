// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A resource outside the caller's project does not exist, so a read of it
//! answers 404 rather than 403.

use pistack_types::{ApiError, ApiResult, Caller, Kind};

pub(super) fn visible<K: Kind>(caller: &Caller, r: K) -> ApiResult<K> {
    let p = &r.meta().project;
    if caller.is_admin() || p.is_none() || *p == caller.project {
        Ok(r)
    } else {
        Err(ApiError::not_found(format!(
            "{} {} not found",
            K::KIND,
            r.meta().id
        )))
    }
}
