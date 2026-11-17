// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How well a route's host matches the request's: exact beats wildcard
//! beats no host at all.

/// 2 for an exact host, 1 for a wildcard, 0 for a route with no host.
pub(super) fn host_rank(route: Option<&str>, request: Option<&str>) -> Option<u8> {
    let Some(want) = route else {
        return Some(0);
    };
    let have = request?;
    if let Some(suffix) = want.strip_prefix('*') {
        let label = have.strip_suffix(suffix)?;
        return (!label.is_empty() && !label.contains('.')).then_some(1);
    }
    (want == have).then_some(2)
}
