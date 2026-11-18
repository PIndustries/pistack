// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Apex names are absolute and lower-case in the store, but a NOTIFY arrives
/// from the wire and either side may have been written without the trailing
/// dot.
pub(crate) fn same_apex(a: &str, b: &str) -> bool {
    let t = |s: &str| s.trim_end_matches('.').to_ascii_lowercase();
    t(a) == t(b)
}
