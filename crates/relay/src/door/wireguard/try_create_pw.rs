// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Creating the `pw` WireGuard interface before `wg set` runs against it.

use super::super::iface::is_pw;
use super::ip_bin::ip_bin;

/// Create only `iface`, which has already been checked as `pw` plus 12 hex
/// digits. A missing `ip` is ignored. The command never deletes and never
/// names `wg-fleet`.
pub(crate) async fn try_create_pw(iface: &str) {
    if !is_pw(iface) {
        return;
    }
    let Some(bin) = ip_bin() else {
        return;
    };
    let args = ["link", "add", "dev", iface, "type", "wireguard"];
    if args.iter().any(|arg| {
        *arg == "wg-fleet" || *arg == "del" || *arg == "delete" || arg.contains("wg-fleet")
    }) {
        return;
    }
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _ = crate::spawn_retry::status_retrying_busy(&mut cmd).await;
}
