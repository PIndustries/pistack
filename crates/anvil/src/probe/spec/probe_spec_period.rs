// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::consts::DEFAULT_PERIOD;
use crate::probe::nonzero::nonzero;
use crate::probe::spec::probe_spec::ProbeSpec;

impl ProbeSpec {
    pub(crate) fn period(&self) -> u32 {
        nonzero(self.period_seconds, DEFAULT_PERIOD)
    }
}
