// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use tokio::sync::oneshot;

use crate::proto::{CommandId, CommandKind};

use super::command_queue::CommandQueue;
use super::outcome::Outcome;

/// Handle to one enqueued command; await it for the agent's result.
#[must_use = "a Ticket that is never awaited still times out, but nobody learns the result"]
pub struct Ticket {
    pub(crate) id: CommandId,
    pub(crate) node: String,
    pub(crate) kind: CommandKind,
    pub(crate) queue: CommandQueue,
    pub(crate) rx: oneshot::Receiver<Outcome>,
}
