// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::book_type::Book;
use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;

impl Book {
    pub(super) fn record(&mut self, check: &DueCheck, outcome: &Outcome) {
        let Some(machine) = self.machines.get_mut(&check.machine) else {
            return;
        };
        if machine.generation != check.generation {
            return;
        }
        if let Some(track) = machine
            .tracks
            .iter_mut()
            .find(|t| t.container == check.container && t.kind.as_str() == check.kind)
        {
            track.note(outcome);
        }
    }
}
