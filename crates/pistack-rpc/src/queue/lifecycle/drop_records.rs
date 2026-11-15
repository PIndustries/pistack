// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Remove records from the store in one write. A failure is logged and
    /// left: a record nobody owns goes at the next sweep, and a server that
    /// is no longer the leader cannot write at all.
    pub(crate) fn drop_records(&self, keys: Vec<String>) {
        let Some(store) = self.0.store.get() else {
            return;
        };
        if let Err(e) = store.delete_meta_many(&keys) {
            tracing::debug!(error = %e, count = keys.len(), "command records were not removed");
        }
    }
}
