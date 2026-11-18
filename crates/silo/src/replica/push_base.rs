// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

/// A peer asked to pull is given this long, plus a second for each
/// mebibyte, before the next one is tried.
pub(crate) const PUSH_BASE: Duration = Duration::from_secs(30);
