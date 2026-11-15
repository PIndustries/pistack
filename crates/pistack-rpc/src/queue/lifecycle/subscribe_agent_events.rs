// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use tokio::sync::broadcast;

use crate::proto::AgentEvent;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Every hello and heartbeat the queue records, as it records it.
    /// `forge` uses this to register hosts and refresh `last_seen`.
    pub fn subscribe_agent_events(&self) -> broadcast::Receiver<AgentEvent> {
        self.0.agent_events.subscribe()
    }
}
