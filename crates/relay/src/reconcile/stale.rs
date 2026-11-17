// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Whether a resource still wants a pass: its status lags its revision, or
//! the last one failed.

use pistack_types::Phase;

pub(crate) fn stale(observed: u64, revision: u64, phase: Phase) -> bool {
    observed != revision || phase == Phase::Error
}
