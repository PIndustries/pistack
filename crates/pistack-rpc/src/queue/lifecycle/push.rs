// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use tokio::sync::oneshot;
use tokio::time::Instant;

use crate::error::RpcError;
use crate::proto::Command;
use crate::queue::command_queue::CommandQueue;
use crate::queue::outcome::Outcome;
use crate::queue::pending::Pending;

impl CommandQueue {
    /// Put `cmd` on the node's FIFO and track it as pending.
    pub(crate) fn push(
        &self,
        node: &str,
        cmd: Command,
        enqueued: Instant,
        record: Option<String>,
        waiter: oneshot::Sender<Outcome>,
    ) -> Result<(), RpcError> {
        let (id, kind, issued_at) = (cmd.id, cmd.kind, cmd.issued_at);
        let mut st = self.0.state.lock();
        let e = self.entry(&mut st, node);
        e.tx.try_send(cmd).map_err(|_| RpcError::QueueFull {
            node: node.to_owned(),
        })?;
        if let Some(record) = &record {
            st.keyed.insert(record.clone(), id);
        }
        st.pending.insert(
            id,
            Pending {
                node: node.to_owned(),
                kind,
                issued_at,
                enqueued,
                delivered: false,
                waiter: Some(waiter),
                record,
            },
        );
        Ok(())
    }
}
