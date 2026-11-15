// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::due_check::DueCheck;
use crate::probe::book::probe_book::ProbeBook;
use crate::probe::book::probe_target::ProbeTarget;
use chrono::{DateTime, Utc};

impl ProbeBook {
    /// Checks whose period has arrived. Marks them attempted so a slow
    /// check does not start again before the next period.
    pub fn due(&self, targets: &[ProbeTarget], now: DateTime<Utc>) -> Vec<DueCheck> {
        self.lock().due(targets, now)
    }
}
