// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::collections::HashSet;

use chrono::Utc;

use crate::queue::command_queue::CommandQueue;
use crate::queue::consts::COMMAND_RECORD_PREFIX;
use crate::queue::record::Record;

impl CommandQueue {
    /// Remove records that are older than `command_timeout` and belong to
    /// no pending command here: what an earlier leader left for commands
    /// nobody issued again. Returns how many went.
    pub fn sweep_records(&self) -> usize {
        let Some(store) = self.0.store.get() else {
            return 0;
        };
        let Ok(rows) = store.raw_scan("meta", COMMAND_RECORD_PREFIX) else {
            return 0;
        };
        let held: HashSet<String> = self.0.state.lock().keyed.keys().cloned().collect();
        let now = Utc::now();
        let timeout = self.0.cfg.command_timeout;
        let stale: Vec<String> = rows
            .into_iter()
            .filter(|(key, _)| !held.contains(key))
            .filter(|(_, bytes)| match serde_json::from_slice::<Record>(bytes) {
                Ok(r) => (now - r.issued_at).to_std().is_ok_and(|age| age >= timeout),
                Err(_) => true,
            })
            .map(|(key, _)| key)
            .collect();
        let n = stale.len();
        self.drop_records(stale);
        n
    }
}
