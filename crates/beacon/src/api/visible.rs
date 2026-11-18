// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult, Caller, Kind};

pub(crate) fn visible<K: Kind>(caller: &Caller, r: K) -> ApiResult<K> {
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
