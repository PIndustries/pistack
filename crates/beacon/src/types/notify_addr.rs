// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// Check one address written `ip` or `ip:port`, and return it in the form
/// [`crate::notify::parse_addr`] reads: trimmed, a v6 host bracketed only when a
/// port follows. A bare address names the standard port, so a v6 address with
/// several colons and no bracket is the address itself, never a malformed
/// `ip:port` pair. An address whose octets or port do not parse is refused here,
/// where the caller can answer, rather than deep in a notify pass.
pub(crate) fn notify_addr(raw: &str) -> ApiResult<String> {
    let a = raw.trim();
    let bad = |why: &str| ApiError::bad_request(format!("{raw} {why}"));
    // Split into host and port on the last colon. A bracketed host always ends
    // before its port; otherwise only a single colon separates the two, so a
    // v6 address written bare — several colons, no bracket — is all host.
    let bracketed = a.starts_with('[');
    let split = match a.rfind(':') {
        Some(i) if bracketed => Some(i),
        Some(i) if a.matches(':').count() == 1 => Some(i),
        Some(_) => None,
        None => None,
    };
    let (host, port) = match split {
        Some(i) => {
            let (h, p) = (&a[..i], &a[i + 1..]);
            if p.parse::<u16>().is_err() {
                return Err(bad("has a port that is not a number"));
            }
            (h, Some(p))
        }
        None => (a, None),
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.parse::<std::net::IpAddr>().is_err() {
        return Err(bad("is not an ip address"));
    }
    Ok(match port {
        Some(p) if host.contains(':') => format!("[{host}]:{p}"),
        Some(p) => format!("{host}:{p}"),
        None => host.to_owned(),
    })
}
