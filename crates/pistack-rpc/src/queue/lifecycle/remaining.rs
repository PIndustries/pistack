// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use crate::proto::CommandId;
use crate::queue::command_queue::CommandQueue;

impl CommandQueue {
    /// Time left before `id` times out; zero if it is not pending.
    pub(crate) fn remaining(&self, id: CommandId) -> Duration {
        let st = self.0.state.lock();
        match st.pending.get(&id) {
            Some(p) => self
                .0
                .cfg
                .command_timeout
                .saturating_sub(p.enqueued.elapsed()),
            None => Duration::ZERO,
        }
    }
}
