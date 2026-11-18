// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// How long to wait for a power-off to be confirmed before the evacuation
/// is refused. A host that will not answer its BMC within this is treated
/// as unfenceable, and nothing moves.
pub const FENCE_TIMEOUT: Duration = Duration::from_secs(60);
