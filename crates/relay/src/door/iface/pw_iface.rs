// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The WireGuard interface name for a listener id: `pw` plus its hex.

use super::device_hex::device_hex;
use super::is_pw::is_pw;
use pistack_types::{ApiError, ApiResult};

/// `pw` plus 12 hex digits of `id`. Never `wg-fleet` and never a name that
/// starts with `pi`.
pub(crate) fn pw_iface(id: &str) -> ApiResult<String> {
    let hex = device_hex(id)?;
    let name = format!("pw{hex}");
    if !is_pw(&name) {
        return Err(ApiError::bad_request(
            "wireguard interface must be pw plus 12 hex digits",
        ));
    }
    Ok(name)
}
