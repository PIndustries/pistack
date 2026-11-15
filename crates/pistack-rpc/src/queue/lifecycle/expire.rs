// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::proto::CommandId;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Expire one command now, regardless of age. Used by [`Ticket::wait`].
    pub(crate) fn expire(&self, id: CommandId) {
        let p = self.0.state.lock().forget(id);
        if let Some(p) = p {
            tracing::warn!(node = %p.node, %id, kind = %p.kind, "command timed out");
            if let Some(record) = p.record {
                self.drop_records(vec![record]);
            }
        }
    }
}
