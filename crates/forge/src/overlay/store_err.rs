// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;

pub(in crate::overlay) fn store_err(e: pistack_store::StoreError) -> ApiError {
    ApiError::internal(e.to_string())
}
