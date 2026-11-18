// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// Full pass interval, catching anything an event missed.
pub(crate) const RESYNC: Duration = Duration::from_secs(60);
