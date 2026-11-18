// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::driver::DriverRegistry;
use crate::scheduler::Strategy;
use pistack_store::Store;
use std::collections::BTreeMap;
use std::sync::Arc;

use super::reconciler::Reconciler;

impl Reconciler {
    pub fn new(store: Store, drivers: Arc<DriverRegistry>, strategy: Strategy) -> Self {
        Self {
            store,
            drivers,
            strategy,
            queue: None,
            overlay_sent: Arc::new(parking_lot::Mutex::new(BTreeMap::new())),
        }
    }
}
