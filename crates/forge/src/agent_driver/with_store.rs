// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::rpc_driver::RpcDriver;
use pistack_store::Store;
impl RpcDriver {
    /// Resolve sizes through `store` so the agent gets real resources.
    pub fn with_store(mut self, store: Store) -> Self {
        self.store = Some(store);
        self
    }
}
