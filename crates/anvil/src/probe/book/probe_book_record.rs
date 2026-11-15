// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::outcome::Outcome;
use crate::probe::book::probe_book::ProbeBook;

impl ProbeBook {
    pub fn record(&self, check: &DueCheck, outcome: &Outcome) {
        self.lock().record(check, outcome);
    }
}
