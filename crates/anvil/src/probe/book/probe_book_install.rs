// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;
use crate::probe::spec::container_probes::ContainerProbes;
use chrono::{DateTime, Utc};
use pistack_types::Id;
impl ProbeBook {
    /// Replace the streaks for `id`. An empty list forgets the machine.
    /// A later [`Self::record`] from a check issued before this call is
    /// ignored.
    pub fn install(&self, id: &Id, specs: Vec<ContainerProbes>, now: DateTime<Utc>) {
        self.lock().install(id, specs, now);
    }
}
