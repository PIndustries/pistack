// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A follower campaigns when the leader has been silent this long.
pub fn should_campaign(silent: Duration, timeout: Duration) -> bool {
    silent >= timeout
}
