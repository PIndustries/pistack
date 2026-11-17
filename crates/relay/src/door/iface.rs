// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The WireGuard interface name a door may use, and the argv guard that
//! keeps it to that name.

use pistack_types::ApiError;

/// `pw` plus 12 hex digits of `id`. Never `wg-fleet` and never a name that
/// starts with `pi`.
pub(super) fn pw_iface(id: &str) -> Result<String, ApiError> {
    let hex = device_hex(id)?;
    let name = format!("pw{hex}");
    if !is_pw(&name) {
        return Err(ApiError::bad_request(
            "wireguard interface must be pw plus 12 hex digits",
        ));
    }
    Ok(name)
}

pub(super) fn is_pw(name: &str) -> bool {
    let Some(hex) = name.strip_prefix("pw") else {
        return false;
    };
    name != "wg-fleet"
        && !name.starts_with("pi")
        && hex.len() == 12
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && hex.chars().all(|c| !c.is_ascii_uppercase())
}

/// Twelve lowercase hex digits. Split on the first `-`. When the left side
/// is all ASCII letters and the right side is non-empty, only the right
/// side is used.
pub(super) fn device_hex(id: &str) -> Result<String, ApiError> {
    let source = match id.split_once('-') {
        Some((kind, rest)) if !rest.is_empty() && kind.chars().all(|c| c.is_ascii_alphabetic()) => {
            rest
        }
        _ => id,
    };
    let mut hex = String::with_capacity(12);
    for c in source.chars() {
        if c.is_ascii_hexdigit() {
            hex.push(c.to_ascii_lowercase());
            if hex.len() == 12 {
                return Ok(hex);
            }
        }
    }
    Err(ApiError::bad_request(format!(
        "{id} does not yield 12 hex digits"
    )))
}

pub(super) fn refuse_wg_args(argv: &[String]) -> Result<(), ApiError> {
    for arg in argv {
        if arg.contains("wg-fleet") || arg == "del" || arg == "delete" || arg == "down" {
            return Err(ApiError::bad_request(
                "refusing to name, configure, or delete wg-fleet",
            ));
        }
    }
    let iface = argv.get(1).map(String::as_str).unwrap_or("");
    if !is_pw(iface) {
        return Err(ApiError::bad_request(
            "wireguard interface must be pw plus 12 hex digits",
        ));
    }
    Ok(())
}
