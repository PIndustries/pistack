// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A host whose last overlay apply failed gets the same plan again after
/// this long, even when nothing in the plan changed.
pub const OVERLAY_RETRY: Duration = Duration::from_secs(30);
