// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::sync::Arc;

use tokio::sync::mpsc;

use crate::queue::command_queue::CommandQueue;
use crate::queue::nodes::node_entry::NodeEntry;
use crate::queue::nodes::node_info::NodeInfo;
use crate::queue::nodes::state::State;

impl CommandQueue {
    pub(crate) fn entry<'a>(&self, st: &'a mut State, node: &str) -> &'a mut NodeEntry {
        let capacity = self.0.cfg.capacity;
        st.nodes.entry(node.to_owned()).or_insert_with(|| {
            let (tx, rx) = mpsc::channel(capacity);
            NodeEntry {
                tx,
                rx: Arc::new(tokio::sync::Mutex::new(rx)),
                info: NodeInfo::new(node),
            }
        })
    }
}
