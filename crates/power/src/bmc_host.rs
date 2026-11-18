// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

/// A bare `host` or `host:port` BMC address, checked so it is safe to pass
/// to a tool as one argument.
pub(crate) fn bmc_host(raw: &str) -> ApiResult<(String, Option<u16>)> {
    let raw = raw.trim();
    if raw.is_empty()
        || raw.len() > 253
        || raw.starts_with('-')
        || raw.chars().any(|c| {
            c.is_whitespace()
                || c.is_control()
                || matches!(c, '/' | ';' | '|' | '&' | '$' | '`' | '\\' | '"' | '\'')
        })
    {
        return Err(ApiError::bad_request("bmc address is not a host"));
    }
    if let Some((host, port)) = raw.rsplit_once(':') {
        if !host.is_empty() && !host.contains(':') && port.chars().all(|c| c.is_ascii_digit()) {
            if let Ok(port) = port.parse::<u16>() {
                if port > 0 {
                    return Ok((host.to_owned(), Some(port)));
                }
            }
        }
    }
    Ok((raw.to_owned(), None))
}
