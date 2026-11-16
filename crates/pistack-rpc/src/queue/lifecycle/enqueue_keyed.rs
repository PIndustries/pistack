// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use chrono::Utc;
use tokio::sync::oneshot;
use tokio::time::Instant;
use uuid::Uuid;

use crate::error::RpcError;
use crate::proto::{Command, CommandKind};
use crate::queue::command_queue::CommandQueue;
use crate::queue::record::Record;
use crate::queue::record_key::record_key;
use crate::queue::ticket::Ticket;

impl CommandQueue {
    /// Queue a command that must not be lost or repeated when the leader
    /// changes. `key` names what the command acts on (`machine/<id>`); a
    /// node has at most one outstanding command per key.
    ///
    /// The id is recorded in the store. Issuing the same kind for the same
    /// key again, here or on the next leader, gives the command the id it
    /// had, for as long as the first would have been waited for. The agent
    /// answers a repeat of an id it has completed without running it, so
    /// the caller gets the first run's result. A different kind for the
    /// key replaces what was there and gets a new id. An earlier ticket for
    /// the key fails with [`RpcError::Closed`].
    pub fn enqueue_keyed(
        &self,
        node: &str,
        key: &str,
        kind: CommandKind,
        payload: serde_json::Value,
    ) -> Result<Ticket, RpcError> {
        self.reap();
        let record = record_key(node, key);
        let _one = self.0.keyed_lock.lock();
        let (tx, rx) = oneshot::channel();
        {
            let mut st = self.0.state.lock();
            if let Some(id) = st.keyed.get(&record).copied() {
                match st.pending.get_mut(&id) {
                    // Already queued or with the agent: wait for that one.
                    Some(p) if p.kind == kind => {
                        p.waiter = Some(tx);
                        return Ok(self.ticket(id, node, kind, rx));
                    }
                    _ => {
                        st.forget(id);
                    }
                }
            }
        }
        let now = Utc::now();
        let timeout = self.0.cfg.command_timeout;
        let store = self.0.store.get();
        let earlier = store
            .and_then(|s| s.get_meta::<Record>(&record).ok().flatten())
            .filter(|r| r.kind == kind)
            .and_then(|r| {
                let age = (now - r.issued_at).to_std().unwrap_or_default();
                (age < timeout).then_some((r, age))
            });
        let adopted = earlier.is_some();
        let (id, issued_at, age) = match earlier {
            Some((r, age)) => (r.id, r.issued_at, age),
            None => {
                let id = Uuid::now_v7();
                if let Some(s) = store {
                    let rec = Record {
                        id,
                        kind,
                        issued_at: now,
                    };
                    s.put_meta(&record, &rec)
                        .map_err(|e| RpcError::Store(e.to_string()))?;
                }
                (id, now, Duration::ZERO)
            }
        };
        let cmd = Command {
            id,
            kind,
            payload,
            issued_at,
            remember: true,
        };
        let enqueued = Instant::now();
        let enqueued = enqueued.checked_sub(age).unwrap_or(enqueued);
        if let Err(e) = self.push(node, cmd, enqueued, Some(record.clone()), tx) {
            if !adopted {
                self.drop_records(vec![record]);
            }
            return Err(e);
        }
        tracing::debug!(node, %id, %kind, key, adopted, "keyed command enqueued");
        Ok(self.ticket(id, node, kind, rx))
    }
}
