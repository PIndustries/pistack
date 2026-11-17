// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A store error as the api error the reconciler's status write carries.

use pistack_store::StoreError;

pub(crate) fn store_err(e: StoreError) -> pistack_types::ApiError {
    e.into()
}
