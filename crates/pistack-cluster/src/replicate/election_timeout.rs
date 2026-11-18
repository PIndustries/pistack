// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use super::election_timeout_min::ELECTION_TIMEOUT_MIN;
use super::election_timeout_span::ELECTION_TIMEOUT_SPAN;

/// Wait in `[ELECTION_TIMEOUT_MIN, ELECTION_TIMEOUT_MIN + SPAN)`.
pub fn election_timeout(seed: u64) -> Duration {
    let mut x = seed | 1;
    x ^= x << 13;
    x ^= x >> 7;
    x ^= x << 17;
    ELECTION_TIMEOUT_MIN + Duration::from_millis(x % ELECTION_TIMEOUT_SPAN.as_millis() as u64)
}
