// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How a listener is published off this host.
//!
//! [`plan`] returns argv. [`apply`] runs that command when the binary is
//! present and returns unavailable when it is not. WireGuard must not
//! change a device; the existing underlay already carries the listener.
//! Edge publishes on this host and does not run guests.

use pistack_types::ApiError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// Which publisher carries the listener.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DoorKind {
    Cloudflared,
    Tailscale,
    Wireguard,
    Edge,
}

/// A command that would publish the listener. Empty `bin` means do not run
/// anything. `note` says why, when there is no command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DoorPlan {
    pub bin: String,
    pub argv: Vec<String>,
    pub note: String,
}

/// Argv for `kind`. `listen` is a port number for Tailscale and unused for
/// Cloudflared. Upstream must be `http://127.0.0.1:PORT` or an `http://`
/// address in `10.0.0.0/8`, `192.168.0.0/16`, or `172.16.0.0/12` for those
/// two. Whitespace in either string is refused for every kind.
pub fn plan(kind: DoorKind, listen: &str, upstream: &str) -> Result<DoorPlan, ApiError> {
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
        DoorKind::Wireguard => Ok(DoorPlan {
            bin: String::new(),
            argv: Vec::new(),
            note: "the existing underlay carries this listener".into(),
        }),
        DoorKind::Edge => Ok(DoorPlan {
            bin: String::new(),
            argv: Vec::new(),
            note: "this host publishes the listener and does not run guests".into(),
        }),
    }
}

fn listen_port(listen: &str) -> Result<u16, ApiError> {
    match listen.parse::<u16>() {
        Ok(port) if port > 0 => Ok(port),
        _ => Err(ApiError::bad_request(
            "tailscale listen must be a port number",
        )),
    }
}

/// `http://127.0.0.1:PORT` (port required) or `http://` plus an address in
/// 10/8, 192.168/16, or 172.16/12. No path, query, userinfo, or other host.
fn check_upstream(upstream: &str) -> Result<(), ApiError> {
    if allowed_upstream(upstream) {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "upstream must be http://127.0.0.1:PORT or http://10.*, http://192.168.*, or http://172.16-31.*",
        ))
    }
}

struct Slot {
    argv: Vec<String>,
    child: Option<tokio::process::Child>,
    one_shot: bool,
}

fn doors() -> &'static parking_lot::Mutex<HashMap<String, Slot>> {
    static DOORS: OnceLock<parking_lot::Mutex<HashMap<String, Slot>>> = OnceLock::new();
    DOORS.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

fn resolve_bin(kind: DoorKind, fallback: &str) -> Result<PathBuf, ApiError> {
    let key = match kind {
        DoorKind::Cloudflared => "PISTACK_CLOUDFLARED",
        DoorKind::Tailscale => "PISTACK_TAILSCALE",
        DoorKind::Wireguard | DoorKind::Edge => {
            return Err(ApiError::internal("this door has no binary"));
        }
    };
    match std::env::var_os(key) {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(&v);
            if path.is_file() {
                Ok(path)
            } else {
                Err(ApiError::unavailable(format!(
                    "{fallback} is not installed"
                )))
            }
        }
        Some(_) => Err(ApiError::unavailable(format!(
            "{fallback} is not installed"
        ))),
        None => which(fallback).ok_or_else(|| {
            ApiError::unavailable(format!("{fallback} is not installed"))
        }),
    }
}

fn take_slot(id: &str) -> Option<Slot> {
    doors().lock().remove(id)
}

/// Stop the publisher this process started for `id`. WireGuard and edge
/// have nothing to stop.
pub fn stop(id: &str) {
    if let Some(mut slot) = take_slot(id) {
        if let Some(child) = slot.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// Run [`plan`] for `id`. An empty argv is success and starts nothing.
/// A missing cloudflared or tailscale binary is unavailable. Tailscale
/// `serve --bg` must exit 0. Cloudflared is kept running.
pub async fn apply(
    id: &str,
    kind: DoorKind,
    listen: &str,
    upstream: &str,
) -> Result<DoorPlan, ApiError> {
    let plan = plan(kind, listen, upstream)?;
    if plan.bin.is_empty() {
        stop(id);
        return Ok(plan);
    }
    let argv = plan.argv.clone();
    {
        let mut map = doors().lock();
        if let Some(slot) = map.get_mut(id) {
            if slot.argv == argv {
                if slot.one_shot {
                    return Ok(plan);
                }
                if let Some(child) = slot.child.as_mut() {
                    match child.try_wait() {
                        Ok(None) => return Ok(plan),
                        Ok(Some(_)) | Err(_) => {}
                    }
                }
            }
        }
    }
    stop(id);
    let bin = resolve_bin(kind, &plan.bin)?;
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd.spawn().map_err(|e| {
        ApiError::unavailable(format!("{} failed to start: {e}", plan.bin))
    })?;
    match kind {
        DoorKind::Tailscale => {
            let status = match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => {
                    return Err(ApiError::unavailable(format!(
                        "tailscale failed to start: {e}"
                    )));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(ApiError::unavailable("tailscale timed out"));
                }
            };
            if !status.success() {
                return Err(ApiError::unavailable(format!(
                    "tailscale exited {}",
                    status.code().unwrap_or(-1)
                )));
            }
            doors().lock().insert(
                id.to_owned(),
                Slot {
                    argv,
                    child: None,
                    one_shot: true,
                },
            );
        }
        DoorKind::Cloudflared => {
            match child.try_wait() {
                Ok(Some(status)) => {
                    return Err(ApiError::unavailable(format!(
                        "cloudflared exited {}",
                        status.code().unwrap_or(-1)
                    )));
                }
                Ok(None) => {}
                Err(e) => {
                    return Err(ApiError::unavailable(format!(
                        "cloudflared failed to start: {e}"
                    )));
                }
            }
            doors().lock().insert(
                id.to_owned(),
                Slot {
                    argv,
                    child: Some(child),
                    one_shot: false,
                },
            );
        }
        DoorKind::Wireguard | DoorKind::Edge => {}
    }
    Ok(plan)
}

fn allowed_upstream(upstream: &str) -> bool {
    let Some(rest) = upstream.strip_prefix("http://") else {
        return false;
    };
    if rest.is_empty() || rest.contains(['/', '?', '#', '@', '[', ']']) {
        return false;
    }
    let (host, has_port) = match rest.split_once(':') {
        Some((host, port)) => {
            if host.is_empty() || port.is_empty() || port.contains(':') {
                return false;
            }
            match port.parse::<u16>() {
                Ok(0) | Err(_) => return false,
                Ok(_) => (host, true),
            }
        }
        None => (rest, false),
    };
    let Ok(ip) = host.parse::<Ipv4Addr>() else {
        return false;
    };
    if ip == Ipv4Addr::LOCALHOST {
        return has_port;
    }
    let [a, b, _, _] = ip.octets();
    matches!((a, b), (10, _) | (192, 168) | (172, 16..=31))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cloudflared_argv_and_refusals() {
        let got = plan(DoorKind::Cloudflared, "not-a-port", "http://127.0.0.1:8080").unwrap();
        assert_eq!(got.bin, "cloudflared");
        assert!(got.note.is_empty());
        assert_eq!(
            got.argv,
            vec![
                "tunnel",
                "--no-autoupdate",
                "--url",
                "http://127.0.0.1:8080"
            ]
        );
        assert!(!got.argv.iter().any(|a| a == "not-a-port"));

        let ten = plan(DoorKind::Cloudflared, "", "http://10.1.2.3").unwrap();
        assert_eq!(ten.argv[3], "http://10.1.2.3");
        assert!(plan(DoorKind::Cloudflared, "", "http://10.9.8.7:443").is_ok());
        assert!(plan(DoorKind::Cloudflared, "", "http://192.168.1.5").is_ok());
        assert!(plan(DoorKind::Cloudflared, "", "http://192.168.0.1:9").is_ok());
        assert!(plan(DoorKind::Cloudflared, "", "http://172.16.0.1").is_ok());
        assert!(plan(DoorKind::Cloudflared, "", "http://172.31.255.254:80").is_ok());

        let refused = [
            "http://8.8.8.8",
            "http://8.8.8.8:80",
            "https://127.0.0.1:8080",
            "http://127.0.0.1",
            "http://127.0.0.1:0",
            "http://127.0.0.2:80",
            "http://0.0.0.0:80",
            "http://172.15.0.1",
            "http://172.32.0.1",
            "http://192.167.1.1",
            "http://11.0.0.1",
            "http://127.0.0.1:8080/extra",
            "http://127.0.0.1:8080?x=1",
            "http://user@127.0.0.1:8080",
            "http://localhost:8080",
            "http://10.1.2.3:99999",
        ];
        for upstream in refused {
            assert!(
                plan(DoorKind::Cloudflared, "80", upstream).is_err(),
                "{upstream}"
            );
        }
        assert!(plan(DoorKind::Cloudflared, "80", "http://127.0.0.1:8080 ").is_err());
        assert!(plan(DoorKind::Cloudflared, "80", "http://10.0.0.1\n").is_err());
        assert!(plan(DoorKind::Tailscale, "44 3", "http://127.0.0.1:80").is_err());
        assert!(plan(DoorKind::Wireguard, "has space", "http://10.0.0.1").is_err());
        assert!(plan(DoorKind::Edge, "80", "not a url").is_err());
    }

    #[test]
    fn tailscale_serve_argv() {
        let got = plan(DoorKind::Tailscale, "443", "http://10.1.0.2:8080").unwrap();
        assert_eq!(got.bin, "tailscale");
        assert_eq!(
            got.argv,
            vec!["serve", "--bg", "443", "http://10.1.0.2:8080"]
        );
        assert!(got.note.is_empty());
        assert!(plan(DoorKind::Tailscale, "notaport", "http://10.1.0.2:8080").is_err());
        assert!(plan(DoorKind::Tailscale, "0", "http://10.1.0.2:8080").is_err());
        assert!(plan(DoorKind::Tailscale, "443", "http://1.2.3.4:80").is_err());
    }

    #[test]
    fn wireguard_and_edge_do_not_spawn() {
        let wg = plan(DoorKind::Wireguard, "51820", "http://8.8.8.8").unwrap();
        assert!(wg.bin.is_empty());
        assert!(wg.argv.is_empty());
        assert_eq!(wg.note, "the existing underlay carries this listener");

        let edge = plan(DoorKind::Edge, "80", "http://127.0.0.1:80").unwrap();
        assert!(edge.bin.is_empty());
        assert!(edge.argv.is_empty());
        assert_eq!(
            edge.note,
            "this host publishes the listener and does not run guests"
        );
    }

    fn script(dir: &std::path::Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    #[tokio::test]
    async fn apply_runs_cloudflared_fails_closed_and_skips_wireguard() {
        let dir = std::env::temp_dir().join(format!(
            "pistack-door-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let log = dir.join("door.log");
        let hold = script(
            &dir,
            "cloudflared",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_DOOR_LOG\"\nsleep 30\n",
        );
        let once = script(
            &dir,
            "tailscale",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_DOOR_LOG\"\nexit 0\n",
        );
        let _log = std::env::var("PISTACK_DOOR_LOG").ok();
        let _cf = std::env::var("PISTACK_CLOUDFLARED").ok();
        let _ts = std::env::var("PISTACK_TAILSCALE").ok();
        std::env::set_var("PISTACK_DOOR_LOG", &log);
        std::env::set_var("PISTACK_CLOUDFLARED", dir.join("no-such-cloudflared"));
        std::env::set_var("PISTACK_TAILSCALE", &once);

        let missing = apply(
            "door-missing",
            DoorKind::Cloudflared,
            "80",
            "http://127.0.0.1:9",
        )
        .await
        .unwrap_err();
        assert!(
            missing.message.contains("cloudflared is not installed"),
            "{}",
            missing.message
        );

        std::env::set_var("PISTACK_CLOUDFLARED", &hold);
        let id = "door-cf";
        let got = apply(id, DoorKind::Cloudflared, "80", "http://127.0.0.1:9")
            .await
            .unwrap();
        assert_eq!(got.bin, "cloudflared");
        let again = apply(id, DoorKind::Cloudflared, "80", "http://127.0.0.1:9")
            .await
            .unwrap();
        assert_eq!(again.argv, got.argv);
        let mut text = String::new();
        for _ in 0..50 {
            if let Ok(found) = std::fs::read_to_string(&log) {
                if !found.is_empty() {
                    text = found;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert_eq!(
            text.lines().collect::<Vec<_>>(),
            vec!["tunnel", "--no-autoupdate", "--url", "http://127.0.0.1:9"],
            "{text}"
        );
        assert!(text.contains("tunnel"), "{text}");
        assert!(text.contains("http://127.0.0.1:9"), "{text}");
        stop(id);

        let wg_log = dir.join("wg.log");
        std::env::set_var("PISTACK_DOOR_LOG", &wg_log);
        let wg = apply(
            "door-wg",
            DoorKind::Wireguard,
            "51820",
            "http://8.8.8.8",
        )
        .await
        .unwrap();
        assert!(wg.bin.is_empty());
        assert!(!wg_log.exists());
        let edge = apply("door-edge", DoorKind::Edge, "80", "http://127.0.0.1:80")
            .await
            .unwrap();
        assert!(edge.note.contains("does not run guests"));

        std::env::set_var("PISTACK_DOOR_LOG", &log);
        let ts = "door-ts";
        apply(ts, DoorKind::Tailscale, "443", "http://10.1.0.2:8080")
            .await
            .unwrap();
        apply(ts, DoorKind::Tailscale, "443", "http://10.1.0.2:8080")
            .await
            .unwrap();
        let mut text = String::new();
        for _ in 0..50 {
            if let Ok(found) = std::fs::read_to_string(&log) {
                if found.contains("serve") {
                    text = found;
                    break;
                }
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        assert!(text.contains("serve"), "tailscale did not record argv");
        let serves = text.lines().filter(|l| l.contains("serve")).count();
        assert_eq!(serves, 1, "{text}");
        stop(ts);

        match _log {
            Some(v) => std::env::set_var("PISTACK_DOOR_LOG", v),
            None => std::env::remove_var("PISTACK_DOOR_LOG"),
        }
        match _cf {
            Some(v) => std::env::set_var("PISTACK_CLOUDFLARED", v),
            None => std::env::remove_var("PISTACK_CLOUDFLARED"),
        }
        match _ts {
            Some(v) => std::env::set_var("PISTACK_TAILSCALE", v),
            None => std::env::remove_var("PISTACK_TAILSCALE"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
