// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::proto::CommandId;

use super::state::State;
use crate::queue::pending::Pending;

impl State {
    /// Take a command out of `pending`, and its key with it.
    pub(crate) fn forget(&mut self, id: CommandId) -> Option<Pending> {
        let p = self.pending.remove(&id)?;
        if let Some(record) = &p.record {
            if self.keyed.get(record) == Some(&id) {
                self.keyed.remove(record);
            }
        }
        Some(p)
    }
}
