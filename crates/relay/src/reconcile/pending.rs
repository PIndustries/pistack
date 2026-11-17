// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The periodic pass: balancers, listeners and pools still wanting work.

use crate::client::Client;
use crate::reconcile::stale;

impl Client {
    /// Everything not `Ready` (or whose spec moved since the last pass).
    pub async fn reconcile_pending(&self) {
        for lb in self.balancers().list(None).unwrap_or_default() {
            if stale(
                lb.status.observed_revision,
                lb.meta.revision,
                lb.status.phase,
            ) {
                self.reconcile_balancer(&lb.meta.id).await;
            }
        }
        for l in self.listeners().list(None).unwrap_or_default() {
            if stale(l.status.observed_revision, l.meta.revision, l.status.phase) {
                self.reconcile_listener(&l.meta.id).await;
            }
        }
        for p in self.pools().list(None).unwrap_or_default() {
            if stale(p.status.observed_revision, p.meta.revision, p.status.phase) {
                self.reconcile_pool(&p.meta.id).await;
            }
        }
    }
}
