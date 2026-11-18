// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Host of `10.0.0.1:8443`, `http://10.0.0.1:8443/path`, or `[fd00::1]:53`.
pub(crate) fn host_from_hint(raw: &str) -> Option<&str> {
    let raw = raw.trim();
    if raw.is_empty() {
        return None;
    }
    let rest = match raw.split_once("://") {
        Some((_, rest)) => rest,
        None => raw,
    };
    let hostport = rest.split(['/', '?', '#']).next().unwrap_or("");
    if hostport.is_empty() {
        return None;
    }
    if let Some(v6) = hostport.strip_prefix('[') {
        return v6.split(']').next().filter(|s| !s.is_empty());
    }
    // host:port — take everything before the last colon when it looks like
    // an IPv4 or name with a port. A bare IPv4 has no colon.
    if let Some((host, port)) = hostport.rsplit_once(':') {
        if !host.is_empty() && port.chars().all(|c| c.is_ascii_digit()) {
            return Some(host);
        }
    }
    Some(hostport)
}
