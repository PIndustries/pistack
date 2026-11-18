// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::StoreError;
use pistack_types::ApiResult;

pub(crate) fn not_found_to_none<T>(r: pistack_store::Result<T>) -> ApiResult<Option<T>> {
    match r {
        Ok(v) => Ok(Some(v)),
        Err(StoreError::NotFound(_)) => Ok(None),
        Err(e) => Err(e.into()),
    }
}
