// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Forget a node: drop its queue and fail every pending command to it.
    pub fn remove_node(&self, node: &str) {
        self.drop_commands(node);
        let entry = self.0.state.lock().nodes.remove(node);
        drop(entry);
    }
}
