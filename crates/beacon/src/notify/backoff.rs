// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Wait after a missed answer; doubled after every further miss.
pub const BACKOFF: Duration = Duration::from_millis(50);
