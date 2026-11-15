// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::evacuated_off::evacuated_off;
use super::registrar::Registrar;
use crate::types::Host;
use crate::types::Machine;
use pistack_rpc::proto::{CommandKind, MachineRef};
impl Registrar {
    /// reports without owning is left alone.
    pub(super) fn delete_stale_copy(&self, host: &Host, m: &Machine) {
        if !evacuated_off(m, &host.meta.id) {
            return;
        }
        let id = m.meta.id.clone();
        if !self.deleting_stale.lock().insert(id.clone()) {
            return;
        }
        let node = host.spec.node_name.clone();
        let payload = serde_json::json!(MachineRef { id: id.clone() });
        let in_flight = self.deleting_stale.clone();
        let ticket = match self
            .queue
            .enqueue(&node, CommandKind::DeleteMachine, payload)
        {
            Ok(ticket) => ticket,
            Err(e) => {
                in_flight.lock().remove(&id);
                tracing::warn!(machine = %id, node, error = %e, "forge: stale copy delete was not queued");
                return;
            }
        };
        tracing::warn!(
            machine = %id,
            node,
            "forge: host came back holding a machine that was evacuated; deleting its copy"
        );
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            in_flight.lock().remove(&id);
            return;
        };
        // A failed delete is asked for again on a later heartbeat, as long
        // as the host still reports the machine.
        runtime.spawn(async move {
            match ticket.wait().await {
                Ok(r) if r.ok => {}
                Ok(r) => {
                    tracing::warn!(machine = %id, node, error = %r.message, "forge: stale copy was not deleted")
                }
                Err(e) => {
                    tracing::warn!(machine = %id, node, error = %e, "forge: stale copy was not deleted")
                }
            }
            in_flight.lock().remove(&id);
        });
    }
}
