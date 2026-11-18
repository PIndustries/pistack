// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// What one sweep did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Staging dirs looked at.
    pub scanned: usize,
    /// Staging dirs removed because they were past the TTL.
    pub removed: usize,
    /// Expired dirs that could not be removed (logged; retried next sweep).
    pub failed: usize,
}
