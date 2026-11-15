// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::backend::NetBackend;
use crate::client::Client;
use pistack_store::Store;
use std::collections::HashMap;
use std::sync::Arc;
impl Client {
    pub fn new(store: Store, backend: Arc<dyn NetBackend>) -> Self {
        Self {
            store,
            backend,
            port_index: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }
}
