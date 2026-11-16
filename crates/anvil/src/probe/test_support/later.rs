// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Duration as ChronoDuration, Utc};

pub(in crate::probe) fn later(base: DateTime<Utc>, seconds: i64) -> DateTime<Utc> {
    base + ChronoDuration::seconds(seconds)
}
