// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Creating the `pw` WireGuard interface before `wg set` runs against it.
//! Only creates; never deletes, never names `wg-fleet`.

use super::binary::which;
use super::iface::is_pw;
use std::path::PathBuf;

pub(super) fn ip_bin() -> Option<PathBuf> {
    match std::env::var_os("PISTACK_IP") {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            path.is_file().then_some(path)
        }
        _ => {
            // Tests must not create a host interface. Production looks up `ip`.
            if cfg!(test) {
                None
            } else {
                which("ip")
            }
        }
    }
}

/// Create only `iface`, which has already been checked as `pw` plus 12 hex
/// digits. A missing `ip` is ignored. The command never deletes and never
/// names `wg-fleet`.
pub(super) async fn try_create_pw(iface: &str) {
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
