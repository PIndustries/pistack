// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;
use crate::queue::nodes::node_entry::NodeEntry;
use crate::queue::nodes::node_info::NodeInfo;
use crate::queue::nodes::state::State;

impl CommandQueue {
    pub(crate) fn snapshot(&self, st: &State, e: &NodeEntry) -> NodeInfo {
        let mut info = e.info.clone();
        info.queued = self.0.cfg.capacity - e.tx.capacity();
        info.in_flight = st
            .pending
            .values()
            .filter(|p| p.node == info.name && p.delivered)
            .count();
        info
    }
}
