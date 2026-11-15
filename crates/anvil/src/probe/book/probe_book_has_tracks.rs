// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::probe_book::ProbeBook;

impl ProbeBook {
    pub fn has_tracks(&self) -> bool {
        self.lock().machines.values().any(|m| !m.tracks.is_empty())
    }
}
