// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How long a stretch of the request path a route covers, which is what
//! makes the longer route win.

/// Length of the matched route path, or `None` when it does not match.
pub(super) fn path_len(route: &str, exact: bool, request: &str) -> Option<usize> {
    if exact {
        return (route == request).then_some(route.len());
    }
    let base = route.trim_end_matches('/');
    if base.is_empty() {
        return Some(0);
    }
    let matched = request == base
        || request
            .strip_prefix(base)
            .is_some_and(|rest| rest.starts_with('/'));
    matched.then_some(base.len())
}
