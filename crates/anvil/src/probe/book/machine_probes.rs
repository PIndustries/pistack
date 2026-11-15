// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::book::track::Track;

pub(super) struct MachineProbes {
    pub(crate) generation: u64,
    pub(crate) tracks: Vec<Track>,
}
