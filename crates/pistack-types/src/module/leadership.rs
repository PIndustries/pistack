// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Leadership`: whether this process currently holds cluster leadership.

use std::future::Future;
use std::sync::Arc;
use tokio::sync::watch;

use super::CancellationToken;

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
