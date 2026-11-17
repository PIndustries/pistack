// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The root key that unseals a stored private key. Loaded through keep's
//! configured backend; never created here.

use pistack_store::Store;
use pistack_types::{ApiError, ApiResult};

pub(crate) const NO_CERT: &str = "no certificate for this listener";

/// The root key through keep's configured backend; never creates one.
pub(super) fn existing_root(store: &Store) -> ApiResult<keep::RootKey> {
    match keep::RootKey::load_existing(store) {
        Ok(Some(key)) => Ok(key),
        Ok(None) => Err(ApiError::unavailable(NO_CERT)),
        Err(e) => {
            tracing::warn!(error = %e, "relay: cannot load keep root key");
            Err(ApiError::unavailable(NO_CERT))
        }
    }
}
