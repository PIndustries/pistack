// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::proto::AgentEvent;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    pub(crate) fn emit(&self, ev: AgentEvent) {
        // No subscribers is normal (tests, a bare queue); not an error.
        let _ = self.0.agent_events.send(ev);
    }
}
