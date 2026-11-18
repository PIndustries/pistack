// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use parking_lot::RwLock;
use pistack_types::Id;
use std::collections::HashMap;
use std::sync::Arc;

use super::entry::Entry;
use super::host_driver::HostDriver;

/// Host id -> driver. Hosts marked `local` are served in-process (no agent
/// heartbeat), so the reconciler keeps their `last_seen` fresh itself.
#[derive(Default)]
pub struct DriverRegistry {
    pub(crate) inner: RwLock<HashMap<Id, Entry>>,
}

impl DriverRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register the driver that reaches `host`. Replaces any previous one.
    pub fn register(&self, host: Id, driver: Arc<dyn HostDriver>) {
        self.inner.write().insert(
            host,
            Entry {
                driver,
                local: false,
            },
        );
    }

    /// Register a driver that lives in this process (dev mode). The
    /// reconciler heartbeats these hosts itself.
    pub fn register_local(&self, host: Id, driver: Arc<dyn HostDriver>) {
        self.inner.write().insert(
            host,
            Entry {
                driver,
                local: true,
            },
        );
    }

    pub fn unregister(&self, host: &Id) -> bool {
        self.inner.write().remove(host).is_some()
    }

    pub fn get(&self, host: &Id) -> Option<Arc<dyn HostDriver>> {
        self.inner.read().get(host).map(|e| e.driver.clone())
    }

    pub fn is_local(&self, host: &Id) -> bool {
        self.inner
            .read()
            .get(host)
            .map(|e| e.local)
            .unwrap_or(false)
    }

    pub fn hosts(&self) -> Vec<Id> {
        self.inner.read().keys().cloned().collect()
    }

    pub fn len(&self) -> usize {
        self.inner.read().len()
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
