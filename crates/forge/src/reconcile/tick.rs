// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// How often a pass runs even when nothing changed.
pub const TICK: Duration = Duration::from_secs(2);
