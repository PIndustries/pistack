// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::consts::DEFAULT_TIMEOUT;

pub(in crate::probe) fn default_timeout() -> u32 {
    DEFAULT_TIMEOUT
}
