// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The handle every other module holds: the store, the providers and the
//! edge queue, with the builders that set them and the getters that read
//! them back.

use crate::provider::Providers;
use pistack_rpc::CommandQueue;
use pistack_store::Store;

#[derive(Clone)]
pub struct Client {
    pub(super) store: Store,
    pub(super) providers: Providers,
    pub(super) queue: Option<CommandQueue>,
    /// Leave balancers labelled `pistack.io/managed-by=loom` alone in the
    /// reconcile paths. Set on the relay module's background reconciler:
    /// loom binds and closes those sockets on its own providers, so a
    /// second program here would bind the same address twice.
    pub(super) leave_loom: bool,
}

impl Client {
    pub fn new(store: Store, providers: Providers) -> Self {
        Self {
            store,
            providers,
            queue: None,
            leave_loom: false,
        }
    }

    /// Skip loom's balancers when reconciling (see the field doc).
    pub fn leave_loom_managed(mut self) -> Self {
        self.leave_loom = true;
        self
    }

    /// Queue used by an edge door. Without one, that door stays unavailable.
    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    pub fn queue(&self) -> Option<&CommandQueue> {
        self.queue.as_ref()
    }

    pub fn providers(&self) -> &Providers {
        &self.providers
    }
}
