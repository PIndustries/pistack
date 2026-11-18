// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

use super::push_base::PUSH_BASE;

/// How long a peer asked to copy `size` bytes is given.
pub(crate) fn push_wait(size: u64) -> Duration {
    PUSH_BASE + Duration::from_secs(size / (1024 * 1024))
}
