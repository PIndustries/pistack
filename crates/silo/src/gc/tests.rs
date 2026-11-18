// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use super::*;

#[test]
fn interval_tracks_short_ttls_and_caps_long_ones() {
    assert_eq!(
        sweep_interval(Duration::from_secs(30)),
        Duration::from_secs(30)
    );
    assert_eq!(
        sweep_interval(Duration::from_secs(crate::MULTIPART_TTL_SECS)),
        MAX_SWEEP_INTERVAL
    );
    assert_eq!(sweep_interval(Duration::ZERO), Duration::from_secs(1));
}
