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
//!
//! Controllers and reconcile loops must honour [`Leadership`]: they stay idle
//! on a follower, run while this node is the leader, and stop when leadership
//! is lost. API writes are already refused on followers; this keeps in-process
//! writers from mutating a local copy.

use async_trait::async_trait;
use axum::Router;
use std::future::Future;
use std::sync::Arc;
use tokio::sync::{broadcast, watch};
pub use tokio_util::sync::CancellationToken;

use crate::resource::Event;

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

/// Whether this process currently holds cluster leadership.
///
/// Controllers subscribe and only mutate state while [`Leadership::is_leader`]
/// is true. The server flips this when the node is elected or steps down.
#[derive(Clone)]
pub struct Leadership {
    tx: Arc<watch::Sender<bool>>,
}

impl Leadership {
    /// Always the leader. Used by single-process tests and modules that are
    /// started without a cluster handle.
    pub fn always() -> Self {
        Self::new(true)
    }

    /// Start with an explicit flag (true on a sole voter, false on a joiner).
    pub fn new(leader: bool) -> Self {
        let (tx, _rx) = watch::channel(leader);
        Self { tx: Arc::new(tx) }
    }

    /// True when this node should run controllers.
    pub fn is_leader(&self) -> bool {
        *self.tx.borrow()
    }

    /// Publish a leadership change. No-op when the value is unchanged.
    pub fn set(&self, leader: bool) {
        let _ = self.tx.send_if_modified(|cur| {
            if *cur == leader {
                false
            } else {
                *cur = leader;
                true
            }
        });
    }

    /// A receiver that tracks leadership. Clones share the same signal.
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.tx.subscribe()
    }

    /// Block until this node is leader, or `shutdown` fires.
    /// Returns `false` when shutdown won.
    pub async fn wait_until_leader(&self, shutdown: &CancellationToken) -> bool {
        let mut rx = self.subscribe();
        loop {
            if *rx.borrow_and_update() {
                return true;
            }
            tokio::select! {
                _ = shutdown.cancelled() => return false,
                res = rx.changed() => {
                    if res.is_err() {
                        return false;
                    }
                }
            }
        }
    }

    /// Run `make` whenever this node is leader. When leadership is lost the
    /// child token is cancelled and `make` is expected to exit; the next
    /// promotion starts a fresh run. Returns when `shutdown` fires.
    pub async fn run_while_leader<F, Fut>(&self, shutdown: CancellationToken, mut make: F)
    where
        F: FnMut(CancellationToken) -> Fut,
        Fut: Future<Output = ()> + Send + 'static,
    {
        let mut rx = self.subscribe();
        loop {
            if !self.wait_until_leader(&shutdown).await {
                return;
            }
            // Refresh so a demotion that landed during wait is visible.
            let _ = rx.borrow_and_update();
            if !*rx.borrow() {
                continue;
            }
            tracing::info!("controllers: running as leader");
            let child = shutdown.child_token();
            let mut task = tokio::spawn(make(child.clone()));
            let lost = wait_lost_leadership(&mut rx);
            tokio::pin!(lost);
            tokio::select! {
                _ = shutdown.cancelled() => {
                    child.cancel();
                    let _ = (&mut task).await;
                    return;
                }
                res = &mut lost => {
                    let _ = res;
                    tracing::info!("controllers: idle (not leader)");
                    child.cancel();
                    let _ = (&mut task).await;
                    if shutdown.is_cancelled() {
                        return;
                    }
                }
                _ = &mut task => {
                    if shutdown.is_cancelled() || !self.is_leader() {
                        if shutdown.is_cancelled() {
                            return;
                        }
                        continue;
                    }
                    // Controller exited while still leader.
                    return;
                }
            }
        }
    }
}

/// Wait until the watch says we are not leader. Returns a unit so the
/// `Ref` from `wait_for` never crosses an await in the caller.
async fn wait_lost_leadership(
    rx: &mut watch::Receiver<bool>,
) -> Result<(), watch::error::RecvError> {
    rx.wait_for(|leader| !*leader).await.map(|_| ())
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

#[async_trait]
pub trait Module: Send + Sync + 'static {
    fn info(&self) -> ModuleInfo;
    /// Routes relative to the module prefix. The gateway adds auth and
    /// request-id layers; modules add nothing global.
    fn router(&self, ctx: ModuleContext) -> Router;
    /// Spawn long-running tasks. Default: nothing.
    async fn start(&self, _ctx: ModuleContext) -> anyhow::Result<()> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[tokio::test]
    async fn run_while_leader_starts_on_promotion_and_stops_on_demotion() {
        let leadership = Leadership::new(false);
        let shutdown = CancellationToken::new();
        let runs = Arc::new(AtomicUsize::new(0));
        let active = Arc::new(AtomicUsize::new(0));

        let runs_t = runs.clone();
        let active_t = active.clone();
        let lead = leadership.clone();
        let stop = shutdown.clone();
        let task = tokio::spawn(async move {
            lead.run_while_leader(stop, move |child| {
                runs_t.fetch_add(1, Ordering::SeqCst);
                active_t.fetch_add(1, Ordering::SeqCst);
                let active_t = active_t.clone();
                async move {
                    child.cancelled().await;
                    active_t.fetch_sub(1, Ordering::SeqCst);
                }
            })
            .await;
        });

        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 0);
        assert_eq!(active.load(Ordering::SeqCst), 0);

        leadership.set(true);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 1);
        assert_eq!(active.load(Ordering::SeqCst), 1);

        leadership.set(false);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(active.load(Ordering::SeqCst), 0);
        assert_eq!(runs.load(Ordering::SeqCst), 1);

        leadership.set(true);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(runs.load(Ordering::SeqCst), 2);
        assert_eq!(active.load(Ordering::SeqCst), 1);

        shutdown.cancel();
        task.await.unwrap();
        assert_eq!(active.load(Ordering::SeqCst), 0);
    }
}
