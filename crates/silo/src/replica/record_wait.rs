// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// How long a peer asked to pull waits for the object's record to reach
/// it through the log before it gives up.
pub const RECORD_WAIT: Duration = Duration::from_secs(5);
