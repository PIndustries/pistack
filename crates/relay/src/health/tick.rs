// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One pass over every pool: probe the members that are due, fold the
//! result into their status, and report how many flipped.

use super::{apply, probe, Checker};
use pistack_types::{Id, Phase};
use std::time::{Duration, Instant};

impl Checker {
    /// Probe every member that is due (or every member, with `force`).
    /// Returns the number of members whose operating status flipped.
    pub async fn tick(&mut self, force: bool) -> usize {
        let now = Instant::now();
        let mut flipped = 0;
        let pools = self.client.pools().list(None).unwrap_or_default();
        for pool in pools {
            let hm = match &pool.spec.health_monitor {
                Some(hm) if pool.spec.admin_up => hm.clone(),
                _ => continue,
            };
            let interval = Duration::from_secs(hm.interval.max(1) as u64);
            for m in self.client.members_of(&pool.meta.id).unwrap_or_default() {
                if !m.spec.admin_up {
                    continue;
                }
                let due = force
                    || self
                        .last
                        .get(&m.meta.id)
                        .is_none_or(|t| now.duration_since(*t) >= interval);
                if !due {
                    continue;
                }
                self.last.insert(m.meta.id.clone(), now);
                let result = probe(m.addr(), &hm).await;
                let mut next = m.status.clone();
                let flip = apply(&mut next, &hm, &result);
                let changed =
                    flip || next.streak != m.status.streak || next.message != m.status.message;
                if !changed {
                    continue;
                }
                let write = self.client.members().modify(&m.meta.id, |x| {
                    x.status = next.clone();
                    x.status.phase = Phase::Ready;
                    x.status.observed_revision = x.meta.revision + 1;
                    Ok(())
                });
                match write {
                    Ok(_) if flip => {
                        tracing::info!(
                            member = %m.meta.id, addr = %m.addr(), status = ?next.operating_status,
                            "relay: member status changed"
                        );
                        flipped += 1;
                        self.client.touch_pool(&pool.meta.id);
                    }
                    Ok(_) => {}
                    Err(e) => {
                        tracing::warn!(member = %m.meta.id, error = %e, "relay: health status write failed")
                    }
                }
            }
        }
        // Forget members that are gone.
        let live: std::collections::HashSet<Id> = self
            .client
            .members()
            .list(None)
            .unwrap_or_default()
            .into_iter()
            .map(|m| m.meta.id)
            .collect();
        self.last.retain(|id, _| live.contains(id));
        flipped
    }
}
