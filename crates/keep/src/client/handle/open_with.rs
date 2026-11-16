// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::backend::KeyBackend;
use crate::ca::Ca;
use crate::client::Client;
use crate::error::Result;
use crate::quota::KeepConfig;

impl Client {
    /// [`open`](Self::open) with an explicit root-key backend.
    pub fn open_with(store: &Store, backend: &KeyBackend) -> Result<Self> {
        let root_key = backend.load_or_create(store)?;
        let ca = Ca::load_or_init(store, &root_key)?;
        Ok(Self {
            store: store.clone(),
            root_key,
            ca,
            limits: KeepConfig::configured().clone(),
        })
    }
}
