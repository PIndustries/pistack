// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::scheduler::Allocations;
use crate::types::{delete_requested, Machine, Size};
use pistack_types::Resources;
use std::collections::BTreeMap;

use super::reconciler::Reconciler;

impl Reconciler {
    /// One full pass. Public so tests can drive it without the loop.
    pub async fn reconcile_once(&self) -> anyhow::Result<()> {
        let machines = self.machines().list(None)?;
        let sizes: BTreeMap<String, Size> = self
            .sizes()
            .list(None)?
            .into_iter()
            .map(|s| (s.meta.name.clone(), s))
            .collect();
        let size_of = |m: &Machine| -> Option<Resources> {
            sizes
                .get(&m.spec.size)
                .or_else(|| {
                    sizes
                        .values()
                        .find(|s| s.meta.id.to_string() == m.spec.size)
                })
                .map(|s| s.spec.resources)
        };

        let hosts = self.reconcile_hosts(&machines, &size_of)?;

        // Resources committed during this pass but not yet in host.status.free,
        // plus the names already on each host (for affinity hints). Existing
        // placements are already subtracted from status.free by
        // reconcile_hosts, so they contribute names only.
        let mut alloc = Allocations::new();
        for m in &machines {
            if let Some(h) = &m.status.host {
                alloc.add(h, &m.meta.name, Resources::default());
            }
        }

        for m in machines {
            let id = m.meta.id.clone();
            let res = if delete_requested(&m) {
                self.teardown(m).await
            } else {
                self.converge(m, &hosts, &mut alloc, &size_of).await
            };
            if let Err(e) = res {
                if e.to_string() == "not the leader" {
                    tracing::debug!(machine = %id, error = %e, "forge: machine reconcile stopped; not the leader");
                } else {
                    pistack_types::tally::reconcile_failed("forge");
                    tracing::warn!(machine = %id, error = %e, "forge: machine reconcile failed");
                }
            }
        }
        // Binding record for fabric ports. Runs even when the overlay
        // backend is off. A machine with no host does not clear one.
        self.bind_port_hosts()?;
        self.sync_overlay().await;
        Ok(())
    }
}
