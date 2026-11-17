// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use serde_json::Value;

pub(crate) fn listener_protocol(name: &str, value: &Value) -> ApiResult<relay::ListenerProtocol> {
    if let Some(protocol) = value.as_str() {
        return match protocol.to_ascii_lowercase().as_str() {
            "tcp" => Ok(relay::ListenerProtocol::Tcp),
            "udp" => Ok(relay::ListenerProtocol::Udp),
            "http" => Ok(relay::ListenerProtocol::Http),
            "https" => Ok(relay::ListenerProtocol::Https),
            other => Err(ApiError::bad_request(format!(
                "listener {name}: unknown protocol '{other}'"
            ))),
        };
    }
    serde_json::from_value(value.clone())
        .map_err(|e| ApiError::bad_request(format!("listener {name}: protocol: {e}")))
}
