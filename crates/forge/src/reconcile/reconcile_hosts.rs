// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::{Host, Machine};
use chrono::Utc;
use pistack_store::StoreError;
use pistack_types::{Id, Phase, Resources};
use std::collections::BTreeMap;

use super::reconciler::Reconciler;
use super::HOST_STALE_AFTER;

impl Reconciler {
    /// Heartbeat local hosts, mark stale ones, recompute free. Returns the
    /// post-update host list.
    pub(super) fn reconcile_hosts(
        &self,
        machines: &[Machine],
        size_of: &(dyn Fn(&Machine) -> Option<Resources> + Sync),
    ) -> anyhow::Result<Vec<Host>> {
        let col = self.hosts();
        let now = Utc::now();
        let stale = chrono::Duration::from_std(HOST_STALE_AFTER)?;

        let mut used: BTreeMap<Id, Resources> = BTreeMap::new();
        for m in machines {
            if let (Some(h), Some(r)) = (&m.status.host, size_of(m)) {
                let cur = used.entry(h.clone()).or_default();
                *cur = cur.add(&r);
            }
        }

        let mut out = Vec::new();
        for mut h in col.list(None)? {
            let before = h.status.clone();
            if self.drivers.is_local(&h.meta.id) {
                h.status.last_seen = Some(now);
            }
            let unseen = match h.status.last_seen {
                Some(t) => now - t > stale,
                None => true,
            };
            if unseen {
                if h.status.phase != Phase::Error {
                    h.status.phase = Phase::Error;
                    h.status.message = format!("no report for {}s", HOST_STALE_AFTER.as_secs());
                }
            } else if h.status.phase == Phase::Error && h.status.message.starts_with("no report") {
                h.status.phase = Phase::Ready;
                h.status.message.clear();
            }
            h.status.free = h
                .spec
                .capacity
                .saturating_sub(&used.get(&h.meta.id).copied().unwrap_or_default());
            if h.status != before {
                h = match col.update(h, None) {
                    Ok(h) => h,
                    Err(StoreError::NotFound(_)) => continue,
                    Err(e) => return Err(e.into()),
                };
            }
            out.push(h);
        }
        Ok(out)
    }
}
