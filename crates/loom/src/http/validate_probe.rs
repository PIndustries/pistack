// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use pistack_types::{ApiError, ApiResult};

pub(crate) fn validate_probe(container: &str, which: &str, probe: &Probe) -> ApiResult<()> {
    let handlers = [
        probe.http_get.is_some(),
        probe.tcp_socket.is_some(),
        probe.exec.is_some(),
    ]
    .into_iter()
    .filter(|set| *set)
    .count();
    if handlers == 0 {
        return Err(ApiError::bad_request(format!(
            "container '{container}' {which} needs http_get, tcp_socket, or exec"
        )));
    }
    if handlers > 1 {
        return Err(ApiError::bad_request(format!(
            "container '{container}' {which} has more than one handler"
        )));
    }
    if let Some(http) = &probe.http_get {
        if http.port == 0 {
            return Err(ApiError::bad_request(format!(
                "container '{container}' {which} http_get port must be set"
            )));
        }
        if let Some(host) = &http.host {
            if host.trim().is_empty() || host.contains([' ', '/']) {
                return Err(ApiError::bad_request(format!(
                    "container '{container}' {which} http_get host is invalid"
                )));
            }
        }
    }
    if let Some(tcp) = &probe.tcp_socket {
        if tcp.port == 0 {
            return Err(ApiError::bad_request(format!(
                "container '{container}' {which} tcp_socket port must be set"
            )));
        }
    }
    if let Some(exec) = &probe.exec {
        if exec.command.is_empty() || exec.command.iter().any(|c| c.is_empty()) {
            return Err(ApiError::bad_request(format!(
                "container '{container}' {which} exec command is empty"
            )));
        }
        if exec.command.iter().any(|arg| arg.contains('\0')) {
            return Err(ApiError::bad_request(format!(
                "container '{container}' {which} exec command contains a NUL byte"
            )));
        }
    }
    Ok(())
}
