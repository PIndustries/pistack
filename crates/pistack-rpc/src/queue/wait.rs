// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::error::RpcError;
use crate::queue::outcome::Outcome;
use crate::queue::ticket::Ticket;

impl Ticket {
    /// Wait for the result, or [`RpcError::Timeout`] after the queue's
    /// `command_timeout` measured from enqueue.
    pub async fn wait(self) -> Outcome {
        let remaining = self.queue.remaining(self.id);
        let Ticket {
            id,
            node,
            kind,
            queue,
            rx,
        } = self;
        match tokio::time::timeout(remaining, rx).await {
            Ok(Ok(outcome)) => outcome,
            // Sender dropped without a result: the queue itself went away.
            Ok(Err(_)) => Err(RpcError::Closed),
            Err(_) => {
                queue.expire(id);
                Err(RpcError::Timeout { id, kind, node })
            }
        }
    }
}
