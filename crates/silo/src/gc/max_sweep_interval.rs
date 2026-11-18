// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Longest gap between sweeps. Shorter TTLs sweep every TTL.
pub const MAX_SWEEP_INTERVAL: Duration = Duration::from_secs(60 * 60);
