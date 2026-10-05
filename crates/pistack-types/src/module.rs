// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The contract between `pistack-core` and every module.
//!
//! A module is a library crate that exposes:
//! * `info()` – name, API prefix, version,
//! * `router()` – an axum `Router` mounted at `/{prefix}` by the gateway,
//! * `start()` – background work (reconcilers, pollers) run once at boot.
//!
//! Modules never bind ports or open stores themselves; `ModuleContext`
//! hands them everything. That is what makes "one binary, many modules" work.

use async_trait::async_trait;
use axum::Router;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
pub use tokio_util::sync::CancellationToken;

use crate::resource::Event;

/// Whether this control-plane process currently holds leadership.
///
/// Controllers and reconcile loops stay idle unless [`Leadership::is_leader`]
/// is true. They start working again if this node later wins.
#[derive(Clone)]
pub struct Leadership {
    tx: Arc<watch::Sender<bool>>,
    rx: watch::Receiver<bool>,
}

impl Leadership {
    /// A watch that starts as `leader`. Tests and a one-node server use `true`.
    pub fn new(leader: bool) -> Self {
        let (tx, rx) = watch::channel(leader);
        Self {
            tx: Arc::new(tx),
            rx,
        }
    }

    /// True when this process should run controllers and reconcilers.
    pub fn is_leader(&self) -> bool {
        *self.tx.borrow()
    }

    /// Record the current leadership. Watchers wake even when the value
    /// is unchanged, so a reconciler can re-check immediately.
    pub fn set(&self, leader: bool) {
        self.tx.send_replace(leader);
    }

    /// Wait until leadership is written again.
    pub async fn changed(&self) {
        let mut rx = self.rx.clone();
        let _ = rx.changed().await;
    }
}

impl Default for Leadership {
    fn default() -> Self {
        Self::new(true)
    }
}

/// Static description of a module.
#[derive(Clone, Debug)]
pub struct ModuleInfo {
    /// Workshop name: "gate", "forge", "fabric", ...
    pub name: &'static str,
    /// URL prefix the gateway mounts it on, e.g. "identity", "machines".
    /// Plain nouns here on purpose: `pi machine create` -> POST /machines.
    pub prefix: &'static str,
    /// One line, shown by `GET /` discovery and `pi modules`.
    pub summary: &'static str,
    pub version: &'static str,
}

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
    /// Leadership of this process. Reconcilers stay idle unless this node
    /// is the leader, and they run again if it later wins.
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

    /// True when this process should run controllers and reconcilers.
    pub fn is_leader(&self) -> bool {
        self.leadership.is_leader()
    }
}

#[async_trait]
pub trait Module: Send + Sync + 'static {
    fn info(&self) -> ModuleInfo;
    /// Routes relative to the module prefix. The gateway adds auth and
    /// request-id layers; modules add nothing global.
    fn router(&self, ctx: ModuleContext) -> Router;
    /// Spawn long-running tasks. Default: nothing.
    ///
    /// Reconcilers and controllers must stay idle unless
    /// [`ModuleContext::is_leader`] is true, and only run while it stays true.
    async fn start(&self, _ctx: ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn leadership_starts_and_flips() {
        let lead = Leadership::new(false);
        assert!(!lead.is_leader());
        lead.set(true);
        assert!(lead.is_leader());
        lead.set(false);
        assert!(!lead.is_leader());
        assert!(Leadership::default().is_leader());
    }
}
