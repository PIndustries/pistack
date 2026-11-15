// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    pub fn pending_count(&self) -> usize {
        self.0.state.lock().pending.len()
    }
}
