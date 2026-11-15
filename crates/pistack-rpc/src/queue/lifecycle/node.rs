// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;
use crate::queue::nodes::node_info::NodeInfo;

impl CommandQueue {
    /// Snapshot of one node.
    pub fn node(&self, node: &str) -> Option<NodeInfo> {
        let st = self.0.state.lock();
        st.nodes.get(node).map(|e| self.snapshot(&st, e))
    }
}
