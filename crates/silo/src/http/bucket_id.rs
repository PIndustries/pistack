// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult, Id, Spec};

pub(crate) fn bucket_id(bucket: &str) -> ApiResult<Id> {
    Id::parse(BucketSpec::KIND, bucket).ok_or_else(|| ApiError::not_found("no such bucket"))
}
