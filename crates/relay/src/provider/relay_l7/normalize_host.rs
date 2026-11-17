// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! One host name as routes compare it: lower case, no port, no trailing
//! dot.

/// Lower-case, without a port or a trailing dot.
pub(super) fn normalize_host(raw: &str) -> String {
    let raw = raw.trim();
    let host = if let Some(rest) = raw.strip_prefix('[') {
        rest.split(']').next().unwrap_or(rest)
    } else {
        raw.rsplit_once(':')
            .filter(|(_, port)| port.chars().all(|c| c.is_ascii_digit()))
            .map_or(raw, |(host, _)| host)
    };
    host.trim_end_matches('.').to_ascii_lowercase()
}
