// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::outcome::Outcome;
use crate::probe::book::track::Track;

impl Track {
    pub(crate) fn note(&mut self, outcome: &Outcome) {
        if outcome.unsupported {
            self.successes = 0;
            self.failures = 0;
            self.success = false;
            self.unsupported = true;
            self.message = outcome.message.clone();
            return;
        }
        self.unsupported = false;
        if outcome.success {
            self.successes = self.successes.saturating_add(1);
            self.failures = 0;
            self.success = true;
            self.message.clear();
        } else {
            self.failures = self.failures.saturating_add(1);
            self.successes = 0;
            self.success = false;
            self.message = outcome.message.clone();
        }
    }
}
