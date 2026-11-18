// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::evacuated::Evacuated;
use super::left::Left;
use super::proof::Proof;

/// What an evacuation settled on: the proof it used, what it moved, what it
/// left, and a note when a power-off was tried and failed.
pub struct Evacuation {
    pub proof: Proof,
    pub power_off_failed: Option<String>,
    pub evacuated: Vec<Evacuated>,
    pub skipped: Vec<Left>,
}
