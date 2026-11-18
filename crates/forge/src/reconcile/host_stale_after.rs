// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A host that has not reported for this long is `Error`.
pub const HOST_STALE_AFTER: Duration = Duration::from_secs(30);
