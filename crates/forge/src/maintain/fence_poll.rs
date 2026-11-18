// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// How often a pending power-off is re-read.
pub(crate) const FENCE_POLL: Duration = Duration::from_millis(500);
