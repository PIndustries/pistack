// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The host and path a request is routed on: an absolute request target
//! first, otherwise the `Host` header.

use super::normalize_host::normalize_host;

pub(super) fn route_key(head: &[u8]) -> (Option<String>, String) {
    let Ok(text) = std::str::from_utf8(head) else {
        return (None, "/".into());
    };
    let mut lines = text.split("\r\n");
    let target = lines
        .next()
        .and_then(|start| start.split_whitespace().nth(1))
        .unwrap_or("/");
    let mut host = None;
    let mut path = target;
    for scheme in ["http://", "https://"] {
        if let Some(rest) = target.strip_prefix(scheme) {
            let (authority, tail) = match rest.find('/') {
                Some(i) => (&rest[..i], &rest[i..]),
                None => (rest, "/"),
            };
            host = Some(normalize_host(authority));
            path = tail;
        }
    }
    if host.is_none() {
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((name, value)) = line.split_once(':') {
                if name.trim().eq_ignore_ascii_case("host") {
                    host = Some(normalize_host(value));
                    break;
                }
            }
        }
    }
    let path = path.split(['?', '#']).next().unwrap_or("");
    let path = if path.is_empty() { "/" } else { path };
    (host.filter(|h| !h.is_empty()), path.to_owned())
}
