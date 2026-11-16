// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_kind::ProbeKind;
use crate::probe::spec::probe_spec::ProbeSpec;
use chrono::{DateTime, Utc};

#[derive(Clone, Debug)]
pub(super) struct Track {
    pub(crate) container: String,
    pub(crate) kind: ProbeKind,
    pub(crate) spec: ProbeSpec,
    pub(crate) started: DateTime<Utc>,
    pub(crate) last_attempt: Option<DateTime<Utc>>,
    pub(crate) successes: u32,
    pub(crate) failures: u32,
    pub(crate) success: bool,
    pub(crate) unsupported: bool,
    pub(crate) message: String,
}
