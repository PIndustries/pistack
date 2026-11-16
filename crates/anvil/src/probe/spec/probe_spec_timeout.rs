// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::consts::DEFAULT_TIMEOUT;
use crate::probe::nonzero::nonzero;
use crate::probe::spec::probe_spec::ProbeSpec;
use std::time::Duration;

impl ProbeSpec {
    pub(in crate::probe) fn timeout(&self) -> Duration {
        Duration::from_secs(u64::from(nonzero(self.timeout_seconds, DEFAULT_TIMEOUT)))
    }
}
