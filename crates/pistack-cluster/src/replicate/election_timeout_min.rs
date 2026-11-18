// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Shortest wait after the last sign of a leader or candidate before a
/// follower campaigns.
pub const ELECTION_TIMEOUT_MIN: Duration = Duration::from_millis(800);
