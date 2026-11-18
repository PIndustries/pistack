// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::is_loopback::is_loopback;
use super::split_display::split_display;

/// Display number from `virsh vncdisplay`. `:1` is TCP 5901.
/// Only a loopback host is accepted. `0.0.0.0` and every other address are rejected.
pub fn vnc_port(display: &str) -> Result<u16, String> {
    let line = display
        .lines()
        .find(|line| !line.trim().is_empty())
        .unwrap_or("")
        .trim();
    if line.is_empty() {
        return Err("empty vnc display".into());
    }
    let (host, number) = split_display(line)?;
    if !is_loopback(host) {
        return Err("vnc is not on localhost".into());
    }
    let number: u32 = number
        .parse()
        .map_err(|_| "bad vnc display number".to_owned())?;
    let port = 5900u32
        .checked_add(number)
        .ok_or_else(|| "vnc port overflow".to_owned())?;
    u16::try_from(port).map_err(|_| "vnc port overflow".to_owned())
}
