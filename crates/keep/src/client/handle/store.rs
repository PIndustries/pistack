// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::client::Client;

impl Client {
    pub fn store(&self) -> &Store {
        &self.store
    }
}
