// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;
use crate::queue::nodes::node_info::NodeInfo;

impl CommandQueue {
    /// Snapshot of every node, sorted by name.
    pub fn nodes(&self) -> Vec<NodeInfo> {
        let st = self.0.state.lock();
        let mut v: Vec<NodeInfo> = st.nodes.values().map(|e| self.snapshot(&st, e)).collect();
        v.sort_by(|a, b| a.name.cmp(&b.name));
        v
    }
}
