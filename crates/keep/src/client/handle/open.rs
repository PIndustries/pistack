// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::client::Client;
use crate::error::Result;

impl Client {
    /// Open (or on first start, initialise) the root key through the
    /// process's configured backend, and the cloud CA, on `store`.
    pub fn open(store: &Store) -> Result<Self> {
        Self::open_with(store, crate::backend::configured()?)
    }
}
