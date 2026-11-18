// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use super::max_sweep_interval::MAX_SWEEP_INTERVAL;

/// Gap between background sweeps for `ttl`: the TTL, capped at
/// [`MAX_SWEEP_INTERVAL`], never below one second.
pub fn sweep_interval(ttl: Duration) -> Duration {
    ttl.clamp(Duration::from_secs(1), MAX_SWEEP_INTERVAL)
}
