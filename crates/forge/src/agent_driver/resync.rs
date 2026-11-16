// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::registrar::Registrar;
use chrono::Utc;
use pistack_rpc::proto::AgentHello;
impl Registrar {
    /// Treat every node that said hello to the queue a moment ago as
    /// freshly announced: those are the hellos that may have been missed.
    /// An older hello is left alone. Its host is registered already, and
    /// the node may have gone since; announcing it again would mark a dead
    /// host ready.
    pub(super) fn resync(&self) {
        let recent = Utc::now() - crate::reconcile::HOST_STALE_AFTER;
        for n in self.queue.nodes() {
            if n.last_hello.is_none_or(|at| at < recent) {
                continue;
            }
            let hello = AgentHello {
                node_name: n.name,
                capabilities: n.capabilities,
                resources: n.resources_total,
                version: n.version,
                labels: n.labels,
                underlay_ip: n.underlay_ip,
            };
            if let Err(e) = self.on_hello(&hello) {
                tracing::warn!(node = %hello.node_name, error = %e, "forge: resync failed");
            }
        }
    }
}
