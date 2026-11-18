// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult, Caller};

pub(crate) fn visible(caller: &Caller, bucket: Bucket) -> ApiResult<Bucket> {
    let p = &bucket.meta.project;
    if caller.is_admin() || p.is_none() || *p == caller.project {
        Ok(bucket)
    } else {
        Err(ApiError::not_found(format!(
            "bkt {} not found",
            bucket.meta.id
        )))
    }
}
