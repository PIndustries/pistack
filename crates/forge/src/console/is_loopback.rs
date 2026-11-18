// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn is_loopback(host: &str) -> bool {
    host.is_empty()
        || host == "127.0.0.1"
        || host.eq_ignore_ascii_case("localhost")
        || host == "::1"
        || host == "[::1]"
}
