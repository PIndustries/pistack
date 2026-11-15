// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use tokio::sync::oneshot;

use crate::proto::{CommandId, CommandKind};
use crate::queue::command_queue::CommandQueue;
use crate::queue::outcome::Outcome;
use crate::queue::ticket::Ticket;

impl CommandQueue {
    pub(crate) fn ticket(
        &self,
        id: CommandId,
        node: &str,
        kind: CommandKind,
        rx: oneshot::Receiver<Outcome>,
    ) -> Ticket {
        Ticket {
            id,
            node: node.to_owned(),
            kind,
            queue: self.clone(),
            rx,
        }
    }
}
