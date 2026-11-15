// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use crate::driver::DriverRegistry;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use std::sync::Arc;
impl Registrar {
    /// A registrar that records what hosts report into `gauge`'s window.
    pub fn with_gauge(
        store: Store,
        drivers: Arc<DriverRegistry>,
        queue: CommandQueue,
        gauge: gauge::Client,
    ) -> Self {
        Self {
            store,
            drivers,
            queue,
            gauge,
            deleting_stale: Default::default(),
        }
    }
}
