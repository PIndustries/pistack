// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::track::Track;
use chrono::{DateTime, Duration as ChronoDuration, Utc};

impl Track {
    pub(crate) fn due(&self, now: DateTime<Utc>) -> bool {
        let delay = ChronoDuration::seconds(i64::from(self.spec.initial_delay_seconds));
        if now.signed_duration_since(self.started) < delay {
            return false;
        }
        let period = ChronoDuration::seconds(i64::from(self.spec.period()));
        match self.last_attempt {
            None => true,
            Some(at) => now.signed_duration_since(at) >= period,
        }
    }
}
