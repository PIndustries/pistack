// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::kind;
use tokio::sync::broadcast::error::RecvError;

use super::reconciler::Reconciler;
use super::TICK;

impl Reconciler {
    pub(crate) async fn run_loop(&self, active: pistack_types::CancellationToken) {
        // A new term starts from nothing: every host gets the current plan
        // once, so a failed apply remembered by an earlier term (here or on
        // another server) is not forgotten.
        self.overlay_sent.lock().clear();
        let mut rx = self.store.events().subscribe();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = active.cancelled() => break,
                _ = tick.tick() => {}
                ev = rx.recv() => match ev {
                    Ok(e) if e.kind == kind::MACHINE => {}
                    Ok(_) => continue,
                    Err(RecvError::Lagged(n)) => tracing::debug!(skipped = n, "forge: event bus lagged"),
                    Err(RecvError::Closed) => break,
                },
            }
            if let Err(e) = self.reconcile_once().await {
                if e.to_string() == "not the leader" {
                    tracing::debug!(error = %e, "forge: reconcile stopped; not the leader");
                } else {
                    pistack_types::tally::reconcile_failed("forge");
                    tracing::warn!(error = %e, "forge: reconcile pass failed");
                }
            }
        }
        tracing::info!("forge: reconciler stopped");
    }
}
