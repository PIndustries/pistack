// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// `http(s)://host[:port]/path` with nothing clever.
pub fn validate_source_url(url: &str) -> ApiResult<()> {
    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .ok_or_else(|| ApiError::bad_request("url must start with http:// or https://"))?;
    let authority = rest.split(['/', '?', '#']).next().unwrap_or("");
    if authority.is_empty() {
        return Err(ApiError::bad_request("url has no host"));
    }
    if authority.contains('@') || url.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(ApiError::bad_request(
            "url may not contain credentials or whitespace",
        ));
    }
    // Bracketed IPv6 (`[::1]`, `[::1]:443`) must not be split on ':'.
    // A bare `::1` has more than one colon, so it is the host, not host:port.
    let host = if let Some(inner) = authority.strip_prefix('[') {
        inner.split_once(']').map(|(h, _)| h).unwrap_or(inner)
    } else if authority.matches(':').count() > 1 {
        authority
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    if host.is_empty() {
        return Err(ApiError::bad_request("url has no host"));
    }
    if matches!(
        host,
        "localhost" | "127.0.0.1" | "::1" | "[::1]" | "169.254.169.254"
    ) {
        return Err(ApiError::bad_request(
            "url may not point at the server itself",
        ));
    }
    Ok(())
}
