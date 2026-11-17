// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A store error as the api error a write path returns.

use pistack_store::StoreError;
use pistack_types::ApiError;

pub(super) fn store_err(e: StoreError) -> ApiError {
    e.into()
}
