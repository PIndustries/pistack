// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::proto::CommandId;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Returns `false` if the command is no longer pending (expired).
    pub(crate) fn mark_delivered(&self, id: CommandId) -> bool {
        let mut st = self.0.state.lock();
        match st.pending.get_mut(&id) {
            Some(p) => {
                p.delivered = true;
                true
            }
            None => false,
        }
    }
}
