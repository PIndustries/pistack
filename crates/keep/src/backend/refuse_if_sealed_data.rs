// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::backend_err::backend_err;
use super::consts::{META_ROOT_KEY, META_ROOT_KEY_WRAPPED};
use crate::error::Result;
use pistack_store::Store;

/// A store with sealed data but no root key must not get a fresh key: the
/// data would be unreadable and the loss silent.
pub(in crate::backend) fn refuse_if_sealed_data(store: &Store) -> Result<()> {
    for (kind, what) in [("sec/", "secrets"), ("ca/", "certificate authorities")] {
        if !store.raw_scan("resources", kind)?.is_empty() {
            return Err(backend_err(format!(
                "this store holds sealed {what} but no root key ({META_ROOT_KEY} or \
                 {META_ROOT_KEY_WRAPPED}); refusing to generate a new key that could not open \
                 them. Restore the root key from a backup"
            )));
        }
    }
    Ok(())
}
