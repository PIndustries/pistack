// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(crate) fn etag_matches(etag: &str, want: &str) -> bool {
    let want = want.trim().trim_matches('"');
    let etag = etag.trim().trim_matches('"');
    // Weak validators (`W/"…"`) are uncommon for S3 copies; compare the strong form.
    let want = want.strip_prefix("W/").unwrap_or(want).trim_matches('"');
    etag.eq_ignore_ascii_case(want)
}
