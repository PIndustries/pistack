// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::StoreError;
use pistack_types::ApiError;

pub(crate) fn store_err(e: StoreError) -> ApiError {
    e.into()
}
