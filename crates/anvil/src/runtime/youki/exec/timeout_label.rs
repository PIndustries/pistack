// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::time::Duration;

pub(in crate::runtime::youki) fn timeout_label(timeout: Duration) -> String {
    let secs = timeout.as_secs();
    let millis = timeout.subsec_millis();
    if millis == 0 {
        format!("{secs}s")
    } else if secs == 0 {
        format!("{millis}ms")
    } else {
        format!("{secs}.{millis:03}s")
    }
}
