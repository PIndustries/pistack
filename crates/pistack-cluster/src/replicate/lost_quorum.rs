// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use super::election_timeout_min::ELECTION_TIMEOUT_MIN;
use super::election_timeout_span::ELECTION_TIMEOUT_SPAN;

/// A leader that has not heard from a voter majority for this long steps down.
pub fn lost_quorum(since_majority: Duration) -> bool {
    since_majority >= ELECTION_TIMEOUT_MIN + ELECTION_TIMEOUT_SPAN
}
