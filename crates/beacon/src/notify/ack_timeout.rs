// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A peer that has not answered by now is not acknowledged.
pub const ACK_TIMEOUT: Duration = Duration::from_millis(400);
