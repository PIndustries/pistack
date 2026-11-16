// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use crate::driver::DriverRegistry;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use std::sync::Arc;
impl Registrar {
    pub fn new(store: Store, drivers: Arc<DriverRegistry>, queue: CommandQueue) -> Self {
        let gauge = gauge::Client::new(store.clone(), Arc::new(gauge::SampleRing::memory()));
        Self::with_gauge(store, drivers, queue, gauge)
    }
}
