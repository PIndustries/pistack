// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::ApiError;
use std::process::Stdio;

use super::domain_name::domain_name;
use super::virsh_timeout::VIRSH_TIMEOUT;
use super::vnc_port::vnc_port;

/// Run `virsh vncdisplay` and return the loopback TCP port.
pub async fn display_port(machine_id: &str) -> Result<u16, ApiError> {
    let domain = domain_name(machine_id)?;
    let child = tokio::process::Command::new("virsh")
        .args(["-c", "qemu:///system", "vncdisplay", &domain])
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|_| ApiError::unavailable("virsh is not available"))?;
    let finished = tokio::time::timeout(VIRSH_TIMEOUT, child.wait_with_output()).await;
    let output = match finished {
        Ok(Ok(output)) => output,
        Ok(Err(_)) => return Err(ApiError::unavailable("virsh failed")),
        Err(_) => return Err(ApiError::unavailable("virsh timed out")),
    };
    if !output.status.success() {
        return Err(ApiError::unavailable("vnc display is not available"));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    vnc_port(&text).map_err(|_| ApiError::unavailable("vnc display is not on localhost"))
}
