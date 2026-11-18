// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::CommandQueue;

use super::reconciler::Reconciler;

impl Reconciler {
    /// When set, a pass refreshes overlay plans on hosts that have joined.
    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }
}
