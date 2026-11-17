// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The argv each publisher kind builds.

use super::super::iface::pw_iface;
use super::super::iface::refuse_wg_args;
use super::super::kind::DoorKind;
use super::super::listen::listen_port;
use super::super::upstream::check_upstream;
use super::door_plan::DoorPlan;
use pistack_types::{ApiError, ApiResult};

/// Argv for `kind`. `id` names the listener and is the only source of a
/// WireGuard interface name. `listen` is a port number for Tailscale and
/// WireGuard, and unused for Cloudflared. Upstream must be
/// `http://127.0.0.1:PORT` or an `http://` address in `10.0.0.0/8`,
/// `192.168.0.0/16`, or `172.16.0.0/12` for cloudflared and tailscale.
/// Whitespace in either string is refused for every kind.
pub fn plan(kind: DoorKind, id: &str, listen: &str, upstream: &str) -> ApiResult<DoorPlan> {
    if listen.chars().any(|c| c.is_whitespace()) {
        return Err(ApiError::bad_request("listen must not contain whitespace"));
    }
    if upstream.chars().any(|c| c.is_whitespace()) {
        return Err(ApiError::bad_request(
            "upstream must not contain whitespace",
        ));
    }
    match kind {
        DoorKind::Cloudflared => {
            check_upstream(upstream)?;
            Ok(DoorPlan {
                bin: "cloudflared".into(),
                argv: vec![
                    "tunnel".into(),
                    "--no-autoupdate".into(),
                    "--url".into(),
                    upstream.into(),
                ],
                note: String::new(),
            })
        }
        DoorKind::Tailscale => {
            check_upstream(upstream)?;
            let port = listen_port(listen)?;
            Ok(DoorPlan {
                bin: "tailscale".into(),
                argv: vec![
                    "serve".into(),
                    "--bg".into(),
                    port.to_string(),
                    upstream.into(),
                ],
                note: String::new(),
            })
        }
        DoorKind::Wireguard => {
            let iface = pw_iface(id)?;
            let mut argv = vec!["set".to_string(), iface];
            if let Ok(port) = listen.parse::<u16>() {
                if port > 0 {
                    argv.push("listen-port".into());
                    argv.push(port.to_string());
                }
            }
            refuse_wg_args(&argv)?;
            Ok(DoorPlan {
                bin: "wg".into(),
                argv,
                note: String::new(),
            })
        }
        DoorKind::Edge => Ok(DoorPlan {
            bin: String::new(),
            argv: Vec::new(),
            note: "publish is queued for a host whose driver is edge".into(),
        }),
    }
}
