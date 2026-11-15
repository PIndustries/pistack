// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::Utc;
use tokio::sync::oneshot;
use tokio::time::Instant;
use uuid::Uuid;

use crate::error::RpcError;
use crate::proto::{Command, CommandKind};
use crate::queue::command_queue::CommandQueue;
use crate::queue::ticket::Ticket;

impl CommandQueue {
    /// Queue a command for `node`. Fails with [`RpcError::QueueFull`] when
    /// the node has `capacity` undelivered commands already.
    pub fn enqueue(
        &self,
        node: &str,
        kind: CommandKind,
        payload: serde_json::Value,
    ) -> Result<Ticket, RpcError> {
        self.reap();
        let id = Uuid::now_v7();
        let issued_at = Utc::now();
        let cmd = Command {
            id,
            kind,
            payload,
            issued_at,
            remember: false,
        };
        let (tx, rx) = oneshot::channel();
        self.push(node, cmd, Instant::now(), None, tx)?;
        tracing::debug!(node, %id, %kind, "command enqueued");
        Ok(self.ticket(id, node, kind, rx))
    }
}
