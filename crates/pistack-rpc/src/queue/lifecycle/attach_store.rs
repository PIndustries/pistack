// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;

use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Record the ids of keyed commands in `store`, so they outlive this
    /// process and follow the leader. The first store attached stays.
    pub fn attach_store(&self, store: Store) {
        let _ = self.0.store.set(store);
    }
}
