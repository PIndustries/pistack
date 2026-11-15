// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::consts::TIMEOUT;
use crate::runtime::youki::consts::TIMEOUT_ENV;
use std::time::Duration;

pub(in crate::runtime::youki) fn youki_timeout() -> Duration {
    match std::env::var(TIMEOUT_ENV) {
        Ok(raw) => match raw.parse::<u64>() {
            Ok(ms) if ms > 0 => Duration::from_millis(ms),
            _ => TIMEOUT,
        },
        Err(_) => TIMEOUT,
    }
}
