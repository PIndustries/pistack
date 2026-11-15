// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_kind::ProbeKind;
use crate::probe::book::track::Track;
use crate::probe::spec::probe_spec::ProbeSpec;
use chrono::{DateTime, Utc};

pub(super) fn new_track(
    container: String,
    kind: ProbeKind,
    spec: ProbeSpec,
    now: DateTime<Utc>,
) -> Track {
    Track {
        container,
        kind,
        spec,
        started: now,
        last_attempt: None,
        successes: 0,
        failures: 0,
        success: false,
        unsupported: false,
        message: String::new(),
    }
}
