// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;
use crate::queue::pending_info::PendingInfo;

impl CommandQueue {
    /// Every pending command, oldest first.
    pub fn pending(&self) -> Vec<PendingInfo> {
        let st = self.0.state.lock();
        let mut v: Vec<PendingInfo> = st
            .pending
            .iter()
            .map(|(id, p)| PendingInfo {
                id: *id,
                node: p.node.clone(),
                kind: p.kind,
                issued_at: p.issued_at,
                delivered: p.delivered,
            })
            .collect();
        v.sort_by_key(|p| p.id);
        v
    }
}
