// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;
use crate::queue::queue_config::QueueConfig;

impl CommandQueue {
    pub fn config(&self) -> &QueueConfig {
        &self.0.cfg
    }
}
