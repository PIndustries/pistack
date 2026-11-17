// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `ModuleContext`: everything a module may use at runtime.

use std::sync::Arc;
use tokio::sync::broadcast;

use super::leadership::Leadership;
use super::CancellationToken;
use crate::resource::Event;

/// Everything a module may use at runtime. Cheap to clone.
#[derive(Clone)]
pub struct ModuleContext {
    /// The state store, type-erased so this crate does not depend on the
    /// store implementation. Modules call `ctx.store::<pistack_store::Store>()`.
    pub store: Arc<dyn std::any::Any + Send + Sync>,
    /// Cloud-wide event bus. Every resource write publishes here.
    pub events: broadcast::Sender<Event>,
    /// Cancelled on shutdown; reconcilers must watch it.
    pub shutdown: CancellationToken,
    /// Public base URL of this gateway (for self-links and the catalog).
    pub public_url: String,
    /// Node name of the server running this module.
    pub node_name: String,
    /// Cluster leadership. Controllers stay idle unless this node is leader.
    pub leadership: Leadership,
}

impl ModuleContext {
    pub fn store<T: Send + Sync + 'static>(&self) -> Arc<T> {
        self.store
            .clone()
            .downcast::<T>()
            .unwrap_or_else(|_| panic!("store is not the expected type"))
    }
    pub fn publish(&self, ev: Event) {
        // A closed channel just means nobody is listening; not an error.
        let _ = self.events.send(ev);
    }

    /// Context for tests: leadership is held for the life of the process.
    pub fn for_test(
        store: Arc<dyn std::any::Any + Send + Sync>,
        events: broadcast::Sender<Event>,
        shutdown: CancellationToken,
        public_url: impl Into<String>,
        node_name: impl Into<String>,
    ) -> Self {
        Self {
            store,
            events,
            shutdown,
            public_url: public_url.into(),
            node_name: node_name.into(),
            leadership: Leadership::always(),
        }
    }
}
