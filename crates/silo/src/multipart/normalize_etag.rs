// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn normalize_etag(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_start_matches("&quot;")
        .trim_end_matches("&quot;")
        .to_ascii_lowercase()
}
