// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::proto::{CommandResult, ReportAck};
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Deliver a batch of results from `node`. The records of the keyed
    /// commands among them are removed in one store write, before any
    /// waiter hears. In that order a leader that dies in between leaves a
    /// command that is issued again under a new id and run again by the
    /// agent, which every keyed command allows. The other order could leave
    /// the id of a finished command to be handed to a later one.
    pub fn ack_all(&self, node: &str, results: Vec<CommandResult>) -> ReportAck {
        self.reap();
        let mut ack = ReportAck::default();
        let mut done = Vec::new();
        let mut records = Vec::new();
        {
            let mut st = self.0.state.lock();
            for r in results {
                let ours = st.pending.get(&r.id).is_some_and(|p| p.node == node);
                match ours.then(|| st.forget(r.id)).flatten() {
                    Some(p) => {
                        records.extend(p.record.clone());
                        done.push((p, r));
                    }
                    None => ack.unknown.push(r.id),
                }
            }
        }
        if !records.is_empty() {
            self.drop_records(records);
        }
        for (p, result) in done {
            tracing::debug!(node, id = %result.id, kind = %p.kind, ok = result.ok, "command acked");
            ack.accepted += 1;
            if let Some(w) = p.waiter {
                // Receiver gone means nobody cares about the result; fine.
                let _ = w.send(Ok(result));
            }
        }
        ack
    }
}
