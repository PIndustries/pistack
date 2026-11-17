// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Refusing a `wg` argv that would name, configure or delete `wg-fleet`, or
//! that names an interface that is not a `pw` one.

use super::is_pw::is_pw;
use pistack_types::{ApiError, ApiResult};

pub(crate) fn refuse_wg_args(argv: &[String]) -> ApiResult<()> {
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
