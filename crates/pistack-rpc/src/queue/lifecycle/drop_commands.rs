// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::error::RpcError;
use crate::proto::CommandId;
use crate::queue::command_queue::CommandQueue;
use crate::queue::pending::Pending;
use crate::queue::record_key::record_key;

impl CommandQueue {
    /// Fail every pending command to `node` and remove the node's records,
    /// its own and any an earlier leader left. For a host that was removed,
    /// or evacuated: what was asked of it no longer applies. The node stays
    /// known, and commands still on its FIFO are skipped when it polls.
    /// Returns how many pending commands were failed.
    pub fn drop_commands(&self, node: &str) -> usize {
        let failed: Vec<(CommandId, Pending)> = {
            let mut st = self.0.state.lock();
            let ids: Vec<CommandId> = st
                .pending
                .iter()
                .filter(|(_, p)| p.node == node)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| st.forget(id).map(|p| (id, p)))
                .collect()
        };
        if let Some(store) = self.0.store.get() {
            let records: Vec<String> = store
                .raw_scan("meta", &record_key(node, ""))
                .unwrap_or_default()
                .into_iter()
                .map(|(key, _)| key)
                .collect();
            if !records.is_empty() {
                self.drop_records(records);
            }
        }
        let n = failed.len();
        for (id, p) in failed {
            if let Some(w) = p.waiter {
                let _ = w.send(Err(RpcError::NodeRemoved {
                    id,
                    node: node.to_owned(),
                }));
            }
        }
        n
    }
}
