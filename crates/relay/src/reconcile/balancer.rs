// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The balancer pass: realise the VIP, then bring its listeners up.

use crate::client::Client;
use crate::reconcile::stale;
use pistack_types::Id;
use pistack_types::Phase;

impl Client {
    pub async fn reconcile_balancer(&self, id: &Id) {
        let lb = match self.balancers().try_get(id) {
            Ok(Some(lb)) => lb,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(balancer = %id, error = %e, "relay: read failed");
                return;
            }
        };
        if self.leaves(&lb) {
            return;
        }
        if !stale(
            lb.status.observed_revision,
            lb.meta.revision,
            lb.status.phase,
        ) {
            return;
        }
        let result = match self.provider_for(&lb) {
            Ok(p) => p.ensure_balancer(&lb).await,
            Err(e) => Err(e),
        };
        let outcome = self.balancers().modify(id, |b| {
            match &result {
                Ok(r) => {
                    b.status.vip = Some(r.vip);
                    b.status.provider_ref = Some(r.provider_ref.clone());
                    b.status.phase = Phase::Ready;
                    b.status.message.clear();
                }
                Err(e) => {
                    pistack_types::tally::reconcile_failed("relay");
                    b.status.phase = Phase::Error;
                    b.status.message = e.message.clone();
                }
            }
            b.status.observed_revision = b.meta.revision + 1;
            Ok(())
        });
        if let Err(e) = outcome {
            tracing::warn!(balancer = %id, error = %e, "relay: status write failed");
        }
        if result.is_ok() {
            for l in self.listeners_of(id).unwrap_or_default() {
                self.reconcile_listener(&l.meta.id).await;
            }
        }
    }
}
