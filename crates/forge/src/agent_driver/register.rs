// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use super::rpc_driver::RpcDriver;
use crate::types::Host;
use std::sync::Arc;
impl Registrar {
    pub(super) fn register(&self, host: &Host) {
        if self.drivers.is_local(&host.meta.id) {
            // The dev mock host is served in-process; leave it alone.
            return;
        }
        let driver = RpcDriver::for_node(self.queue.clone(), host.spec.node_name.clone())
            .with_store(self.store.clone());
        self.drivers
            .register(host.meta.id.clone(), Arc::new(driver));
    }
}
