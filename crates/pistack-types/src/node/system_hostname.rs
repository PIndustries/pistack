// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `system_hostname`: this machine's host name, made safe as a node name.

use super::sanitize_node_name::sanitize_node_name;

/// This machine's host name, made safe as a node name.
pub fn system_hostname() -> Option<String> {
    for path in ["/etc/hostname", "/proc/sys/kernel/hostname"] {
        if let Some(name) = std::fs::read_to_string(path)
            .ok()
            .and_then(|s| sanitize_node_name(&s))
        {
            return Some(name);
        }
    }
    if let Some(name) = gethostname().and_then(|s| sanitize_node_name(&s)) {
        return Some(name);
    }
    std::env::var("HOSTNAME")
        .ok()
        .and_then(|s| sanitize_node_name(&s))
}

#[cfg(unix)]
fn gethostname() -> Option<String> {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for `buf.len()` bytes, and the result is
    // read only up to the first NUL inside it.
    let rc = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) };
    if rc != 0 {
        return None;
    }
    let end = buf.iter().position(|&b| b == 0).unwrap_or(buf.len());
    std::str::from_utf8(&buf[..end]).ok().map(str::to_owned)
}

#[cfg(not(unix))]
fn gethostname() -> Option<String> {
    None
}
