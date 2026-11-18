// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Pause between attempts, short enough to stay invisible on the happy path.
pub(crate) const SPAWN_RETRY_DELAY: Duration = Duration::from_millis(10);
