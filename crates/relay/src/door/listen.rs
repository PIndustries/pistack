// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `listen` argument as the doors take it: a port number, nothing else.

use pistack_types::ApiError;

pub(super) fn listen_port(listen: &str) -> Result<u16, ApiError> {
    match listen.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(ApiError::bad_request(
            "tailscale listen must be a port number",
        )),
    }
}
