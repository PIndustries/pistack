// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use crate::types::Host;
use chrono::Utc;
use pistack_rpc::proto::Heartbeat;
use pistack_store::StoreError;
use pistack_types::Phase;
impl Registrar {
    /// Refresh `last_seen` and fold machine reports into status.
    pub fn on_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        let Some(host) = self.host_for_node(&hb.node_name)? else {
            // No hello yet (control plane restarted): the ack already told
            // the agent `known = false`, so a hello is on its way.
            tracing::debug!(node = %hb.node_name, "forge: heartbeat from unregistered node");
            return Ok(());
        };
        if self.drivers.get(&host.meta.id).is_none() {
            self.register(&host);
        }
        let col = self.store.collection::<Host>();
        let now = Utc::now();
        match col.modify(&host.meta.id, |x| {
            x.status.last_seen = Some(now);
            x.status.set_ovn(hb.ovn.as_ref());
            if x.status.phase == Phase::Error && x.status.message.starts_with("no report") {
                x.status.phase = Phase::Ready;
                x.status.message.clear();
            }
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        // `status.free` stays with the reconciler, which recomputes it from
        // placements every pass; the agent's figure is informational.
        // Capacity is the host total from hello, not `resources_free`.
        crate::stats::record_heartbeat(&self.gauge, &self.store, &host, hb);
        tracing::trace!(node = %hb.node_name, free = ?hb.resources_free, "forge: heartbeat");
        for r in &hb.machines {
            if let Err(e) = self.apply_report(&host, r) {
                tracing::debug!(machine = %r.id, error = %e, "forge: report not applied");
            }
        }
        Ok(())
    }
}
