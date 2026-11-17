// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pool pass: refresh its online and member counters, then re-program
//! the listeners that send it traffic.

use crate::client::Client;
use crate::reconcile::stale;
use crate::types::OperatingStatus;
use pistack_types::Id;
use pistack_types::Phase;

impl Client {
    /// A pool change (spec or membership) re-programs its listeners and
    /// refreshes the pool's counters.
    pub async fn reconcile_pool(&self, id: &Id) {
        let p = match self.pools().try_get(id) {
            Ok(Some(p)) => p,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!(pool = %id, error = %e, "relay: read failed");
                return;
            }
        };
        if !stale(p.status.observed_revision, p.meta.revision, p.status.phase) {
            return;
        }
        let members = self.members_of(id).unwrap_or_default();
        let online = members
            .iter()
            .filter(|m| {
                matches!(
                    m.status.operating_status,
                    OperatingStatus::Online | OperatingStatus::NoMonitor
                )
            })
            .count() as u32;
        let _ = self.pools().modify(id, |x| {
            x.status.members = members.len() as u32;
            x.status.online = online;
            x.status.phase = Phase::Ready;
            x.status.message.clear();
            x.status.observed_revision = x.meta.revision + 1;
            Ok(())
        });
        for l in self.listeners_using_pool(id).unwrap_or_default() {
            self.program_listener(&l, true).await;
        }
    }

    /// Mark a pool stale so its counters and listeners refresh. Used by the
    /// health checker after a status flip and by member deletes.
    pub fn touch_pool(&self, pool: &Id) {
        let _ = self.pools().modify(pool, |p| {
            // Leaving observed_revision behind the new revision makes the
            // pool stale for the event this write emits.
            p.status.message.clear();
            Ok(())
        });
    }
}
