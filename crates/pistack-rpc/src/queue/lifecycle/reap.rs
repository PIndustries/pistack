// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use tokio::time::Instant;

use crate::error::RpcError;
use crate::proto::CommandId;
use crate::queue::command_queue::CommandQueue;
use crate::queue::consts::SWEEP_EVERY;
use crate::queue::pending::Pending;

impl CommandQueue {
    /// Fail every pending command older than `command_timeout`. Called
    /// automatically by the other operations; call it from a timer too if
    /// the queue can go quiet with waiters outstanding and nobody awaits
    /// tickets. Returns how many expired.
    pub fn reap(&self) -> usize {
        let timeout = self.0.cfg.command_timeout;
        let now = Instant::now();
        let expired: Vec<(CommandId, Pending)> = {
            let mut st = self.0.state.lock();
            let ids: Vec<CommandId> = st
                .pending
                .iter()
                .filter(|(_, p)| now.duration_since(p.enqueued) >= timeout)
                .map(|(id, _)| *id)
                .collect();
            ids.into_iter()
                .filter_map(|id| st.forget(id).map(|p| (id, p)))
                .collect()
        };
        let records: Vec<String> = expired
            .iter()
            .filter_map(|(_, p)| p.record.clone())
            .collect();
        if !records.is_empty() {
            self.drop_records(records);
        }
        let due = {
            let mut swept = self.0.swept.lock();
            let due = swept.is_none_or(|at| now.duration_since(at) >= SWEEP_EVERY);
            if due {
                *swept = Some(now);
            }
            due
        };
        if due {
            self.sweep_records();
        }
        let n = expired.len();
        for (id, p) in expired {
            tracing::warn!(node = %p.node, %id, kind = %p.kind, delivered = p.delivered, "command timed out");
            if let Some(w) = p.waiter {
                let _ = w.send(Err(RpcError::Timeout {
                    id,
                    kind: p.kind,
                    node: p.node,
                }));
            }
        }
        n
    }
}
