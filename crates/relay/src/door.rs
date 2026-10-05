// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! How a listener is published off this host.
//!
//! [`plan`] returns argv. [`apply`] runs that command when the binary is
//! present and returns unavailable when it is not. WireGuard may create
//! only an interface named `pw` plus 12 hex digits, and is unavailable
//! when `wg` is missing. It does not name, configure, or delete `wg-fleet`.
//! Edge enqueues a publish for a host whose driver is `edge` and starts no
//! process. No edge host leaves the listener unavailable.

use pistack_rpc::{CommandKind, CommandQueue};
use pistack_store::Store;
use pistack_types::{kind, ApiError, Resource, Spec};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::Ipv4Addr;
use std::path::PathBuf;
use std::sync::OnceLock;
use std::time::Duration;

/// Store and command queue an edge door reads. Cloudflared, tailscale, and
/// WireGuard ignore it.
#[derive(Clone, Copy)]
pub struct DoorSide<'a> {
    pub store: &'a Store,
    pub queue: Option<&'a CommandQueue>,
}

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

/// Argv for `kind`. `id` names the listener and is the only source of a
/// WireGuard interface name. `listen` is a port number for Tailscale and
/// WireGuard, and unused for Cloudflared. Upstream must be
/// `http://127.0.0.1:PORT` or an `http://` address in `10.0.0.0/8`,
/// `192.168.0.0/16`, or `172.16.0.0/12` for cloudflared and tailscale.
/// Whitespace in either string is refused for every kind.
pub fn plan(kind: DoorKind, id: &str, listen: &str, upstream: &str) -> Result<DoorPlan, ApiError> {
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

/// `pw` plus 12 hex digits of `id`. Never `wg-fleet` and never a name that
/// starts with `pi`.
fn pw_iface(id: &str) -> Result<String, ApiError> {
    let hex = device_hex(id)?;
    let name = format!("pw{hex}");
    if !is_pw(&name) {
        return Err(ApiError::bad_request(
            "wireguard interface must be pw plus 12 hex digits",
        ));
    }
    Ok(name)
}

fn is_pw(name: &str) -> bool {
    let Some(hex) = name.strip_prefix("pw") else {
        return false;
    };
    name != "wg-fleet"
        && !name.starts_with("pi")
        && hex.len() == 12
        && hex.chars().all(|c| c.is_ascii_hexdigit())
        && hex.chars().all(|c| !c.is_ascii_uppercase())
}

/// Twelve lowercase hex digits. Split on the first `-`. When the left side
/// is all ASCII letters and the right side is non-empty, only the right
/// side is used.
fn device_hex(id: &str) -> Result<String, ApiError> {
    let source = match id.split_once('-') {
        Some((kind, rest)) if !rest.is_empty() && kind.chars().all(|c| c.is_ascii_alphabetic()) => {
            rest
        }
        _ => id,
    };
    let mut hex = String::with_capacity(12);
    for c in source.chars() {
        if c.is_ascii_hexdigit() {
            hex.push(c.to_ascii_lowercase());
            if hex.len() == 12 {
                return Ok(hex);
            }
        }
    }
    Err(ApiError::bad_request(format!(
        "{id} does not yield 12 hex digits"
    )))
}

fn refuse_wg_args(argv: &[String]) -> Result<(), ApiError> {
    for arg in argv {
        if arg.contains("wg-fleet") || arg == "del" || arg == "delete" || arg == "down" {
            return Err(ApiError::bad_request(
                "refusing to name, configure, or delete wg-fleet",
            ));
        }
    }
    let iface = argv.get(1).map(String::as_str).unwrap_or("");
    if !is_pw(iface) {
        return Err(ApiError::bad_request(
            "wireguard interface must be pw plus 12 hex digits",
        ));
    }
    Ok(())
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
        DoorKind::Wireguard => "PISTACK_WG",
        DoorKind::Edge => {
            return Err(ApiError::internal("edge starts no process"));
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
        None => which(fallback)
            .ok_or_else(|| ApiError::unavailable(format!("{fallback} is not installed"))),
    }
}

fn take_slot(id: &str) -> Option<Slot> {
    doors().lock().remove(id)
}

/// Stop the publisher this process started for `id`. Edge has no process.
/// WireGuard is not deleted here, and `wg-fleet` is never named.
pub fn stop(id: &str) {
    edge_published().lock().remove(id);
    stop_process(id);
}

fn stop_process(id: &str) {
    if let Some(mut slot) = take_slot(id) {
        if let Some(child) = slot.child.as_mut() {
            let _ = child.start_kill();
        }
    }
}

/// Run [`plan`] for `id`. An empty argv is success and starts nothing,
/// except an edge door, which queues a publish or stays unavailable.
/// A missing cloudflared, tailscale, or wg binary is unavailable. Tailscale
/// `serve --bg` and `wg set` must exit 0. Cloudflared is kept running.
pub async fn apply(
    id: &str,
    kind: DoorKind,
    listen: &str,
    upstream: &str,
    side: DoorSide<'_>,
) -> Result<DoorPlan, ApiError> {
    let plan = plan(kind, id, listen, upstream)?;
    if kind == DoorKind::Edge {
        return apply_edge(id, listen, upstream, plan, side).await;
    }
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
                    if let Ok(None) = child.try_wait() {
                        return Ok(plan);
                    }
                }
            }
        }
    }
    stop_process(id);
    let bin = resolve_bin(kind, &plan.bin)?;
    if kind == DoorKind::Wireguard {
        refuse_wg_args(&plan.argv)?;
        let iface = &plan.argv[1];
        try_create_pw(iface).await;
    }
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    let mut child = cmd
        .spawn()
        .map_err(|e| ApiError::unavailable(format!("{} failed to start: {e}", plan.bin)))?;
    match kind {
        DoorKind::Tailscale | DoorKind::Wireguard => {
            let status = match tokio::time::timeout(Duration::from_secs(15), child.wait()).await {
                Ok(Ok(status)) => status,
                Ok(Err(e)) => {
                    return Err(ApiError::unavailable(format!(
                        "{} failed to start: {e}",
                        plan.bin
                    )));
                }
                Err(_) => {
                    let _ = child.start_kill();
                    return Err(ApiError::unavailable(format!("{} timed out", plan.bin)));
                }
            };
            if !status.success() {
                return Err(ApiError::unavailable(format!(
                    "{} exited {}",
                    plan.bin,
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
        DoorKind::Edge => {}
    }
    Ok(plan)
}

fn ip_bin() -> Option<PathBuf> {
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
async fn try_create_pw(iface: &str) {
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
    let _ = tokio::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .await;
}

struct EdgePublish {
    key: String,
}

fn edge_published() -> &'static parking_lot::Mutex<HashMap<String, EdgePublish>> {
    static EDGE: OnceLock<parking_lot::Mutex<HashMap<String, EdgePublish>>> = OnceLock::new();
    EDGE.get_or_init(|| parking_lot::Mutex::new(HashMap::new()))
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct HostSpecView {
    #[serde(default)]
    node_name: String,
    #[serde(default)]
    drivers: Vec<String>,
}

impl Spec for HostSpecView {
    const KIND: &'static str = kind::HOST;
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct HostStatusView {}

fn edge_node(store: &Store) -> Result<String, ApiError> {
    let hosts = store
        .collection::<Resource<HostSpecView, HostStatusView>>()
        .list(None)
        .map_err(|_| ApiError::unavailable("no edge host is registered"))?;
    let mut nodes: Vec<String> = hosts
        .into_iter()
        .filter(|host| host.spec.drivers.iter().any(|driver| driver == "edge"))
        .map(|host| host.spec.node_name)
        .filter(|name| !name.is_empty() && !name.chars().any(|c| c.is_whitespace()))
        .collect();
    nodes.sort();
    nodes.dedup();
    nodes
        .into_iter()
        .next()
        .ok_or_else(|| ApiError::unavailable("no edge host is registered"))
}

async fn apply_edge(
    id: &str,
    listen: &str,
    upstream: &str,
    mut plan: DoorPlan,
    side: DoorSide<'_>,
) -> Result<DoorPlan, ApiError> {
    let node = edge_node(side.store)?;
    let Some(queue) = side.queue else {
        return Err(ApiError::unavailable("edge publish was not queued"));
    };
    let key = format!("{node}\n{listen}\n{upstream}");
    if edge_published()
        .lock()
        .get(id)
        .is_some_and(|published| published.key == key)
    {
        plan.note = format!("publish queued for {node}");
        return Ok(plan);
    }
    let payload = serde_json::json!({
        "listener": id,
        "listen": listen,
        "upstream": upstream,
    });
    let ticket = queue
        .enqueue(&node, CommandKind::Publish, payload)
        .map_err(|_| ApiError::unavailable("edge publish was not queued"))?;
    let _ticket = ticket;
    stop_process(id);
    edge_published()
        .lock()
        .insert(id.to_owned(), EdgePublish { key });
    plan.note = format!("publish queued for {node}");
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
        let got = plan(
            DoorKind::Cloudflared,
            "cf",
            "not-a-port",
            "http://127.0.0.1:8080",
        )
        .unwrap();
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

        let ten = plan(DoorKind::Cloudflared, "cf", "", "http://10.1.2.3").unwrap();
        assert_eq!(ten.argv[3], "http://10.1.2.3");
        assert!(plan(DoorKind::Cloudflared, "cf", "", "http://10.9.8.7:443").is_ok());
        assert!(plan(DoorKind::Cloudflared, "cf", "", "http://192.168.1.5").is_ok());
        assert!(plan(DoorKind::Cloudflared, "cf", "", "http://192.168.0.1:9").is_ok());
        assert!(plan(DoorKind::Cloudflared, "cf", "", "http://172.16.0.1").is_ok());
        assert!(plan(DoorKind::Cloudflared, "cf", "", "http://172.31.255.254:80").is_ok());

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
                plan(DoorKind::Cloudflared, "cf", "80", upstream).is_err(),
                "{upstream}"
            );
        }
        assert!(plan(DoorKind::Cloudflared, "cf", "80", "http://127.0.0.1:8080 ").is_err());
        assert!(plan(DoorKind::Cloudflared, "cf", "80", "http://10.0.0.1\n").is_err());
        assert!(plan(DoorKind::Tailscale, "ts", "44 3", "http://127.0.0.1:80").is_err());
        assert!(plan(DoorKind::Wireguard, "wg", "has space", "http://10.0.0.1").is_err());
        assert!(plan(DoorKind::Edge, "edge", "80", "not a url").is_err());
    }

    #[test]
    fn tailscale_serve_argv() {
        let got = plan(DoorKind::Tailscale, "ts", "443", "http://10.1.0.2:8080").unwrap();
        assert_eq!(got.bin, "tailscale");
        assert_eq!(
            got.argv,
            vec!["serve", "--bg", "443", "http://10.1.0.2:8080"]
        );
        assert!(got.note.is_empty());
        assert!(plan(
            DoorKind::Tailscale,
            "ts",
            "notaport",
            "http://10.1.0.2:8080"
        )
        .is_err());
        assert!(plan(DoorKind::Tailscale, "ts", "0", "http://10.1.0.2:8080").is_err());
        assert!(plan(DoorKind::Tailscale, "ts", "443", "http://1.2.3.4:80").is_err());
    }

    #[test]
    fn wireguard_names_only_pw_and_edge_starts_nothing() {
        let id = "lsn-0123456789abcdef";
        let wg = plan(DoorKind::Wireguard, id, "51820", "http://8.8.8.8").unwrap();
        assert_eq!(wg.bin, "wg");
        assert_eq!(
            wg.argv,
            vec!["set", "pw0123456789ab", "listen-port", "51820"]
        );
        assert!(wg.argv.iter().all(|arg| !arg.contains("wg-fleet")));
        assert!(is_pw(&wg.argv[1]));
        assert!(plan(DoorKind::Wireguard, "door-wg", "51820", "http://10.0.0.1").is_err());
        assert!(plan(DoorKind::Wireguard, "wg-fleet", "51820", "http://10.0.0.1").is_err());

        let edge = plan(DoorKind::Edge, "edge-1", "80", "http://127.0.0.1:80").unwrap();
        assert!(edge.bin.is_empty());
        assert!(edge.argv.is_empty());
        assert!(edge.note.contains("edge"));
    }

    /// Door tests publish binaries through process environment variables.
    fn env_guard() -> std::sync::MutexGuard<'static, ()> {
        static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
        LOCK.lock().unwrap_or_else(|poison| poison.into_inner())
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
        let _env = env_guard();
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
        let _wg = std::env::var("PISTACK_WG").ok();
        std::env::set_var("PISTACK_WG", dir.join("no-such-wg"));
        std::env::set_var("PISTACK_DOOR_LOG", &log);
        std::env::set_var("PISTACK_CLOUDFLARED", dir.join("no-such-cloudflared"));
        std::env::set_var("PISTACK_TAILSCALE", &once);
        let store = Store::in_memory().unwrap();
        let side = DoorSide {
            store: &store,
            queue: None,
        };

        let missing = apply(
            "door-missing",
            DoorKind::Cloudflared,
            "80",
            "http://127.0.0.1:9",
            side,
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
        let got = apply(id, DoorKind::Cloudflared, "80", "http://127.0.0.1:9", side)
            .await
            .unwrap();
        assert_eq!(got.bin, "cloudflared");
        let again = apply(id, DoorKind::Cloudflared, "80", "http://127.0.0.1:9", side)
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

        let wg_id = "lsn-ffffffffffffffff";
        stop(wg_id);
        let wg_missing = apply(wg_id, DoorKind::Wireguard, "51820", "http://10.0.0.1", side)
            .await
            .unwrap_err();
        assert!(
            wg_missing.message.contains("wg is not installed"),
            "{}",
            wg_missing.message
        );

        let edge = apply(
            "door-edge",
            DoorKind::Edge,
            "80",
            "http://127.0.0.1:80",
            side,
        )
        .await
        .unwrap_err();
        assert_eq!(edge.code, pistack_types::error::ErrorCode::Unavailable);
        assert!(edge.message.contains("no edge host"));

        std::env::set_var("PISTACK_DOOR_LOG", &log);
        let ts = "door-ts";
        apply(ts, DoorKind::Tailscale, "443", "http://10.1.0.2:8080", side)
            .await
            .unwrap();
        apply(ts, DoorKind::Tailscale, "443", "http://10.1.0.2:8080", side)
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
        match _wg {
            Some(v) => std::env::set_var("PISTACK_WG", v),
            None => std::env::remove_var("PISTACK_WG"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn host(store: &Store, name: &str, drivers: &[&str]) {
        store
            .collection::<Resource<HostSpecView, HostStatusView>>()
            .create(Resource::new(
                name,
                None,
                HostSpecView {
                    node_name: name.into(),
                    drivers: drivers.iter().map(|d| (*d).to_string()).collect(),
                },
            ))
            .unwrap();
    }

    #[tokio::test]
    async fn wireguard_creates_only_pw_and_edge_enqueues_publish() {
        let _env = env_guard();
        let dir = std::env::temp_dir().join(format!(
            "pistack-wg-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let wg_log = dir.join("wg.log");
        let ip_log = dir.join("ip.log");
        let wg = script(
            &dir,
            "wg",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nexit 0\n",
                wg_log.display()
            ),
        );
        let ip = script(
            &dir,
            "ip",
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\nexit 0\n",
                ip_log.display()
            ),
        );
        let _wg = std::env::var("PISTACK_WG").ok();
        let _ip = std::env::var("PISTACK_IP").ok();
        std::env::set_var("PISTACK_WG", &wg);
        std::env::set_var("PISTACK_IP", &ip);

        let store = Store::in_memory().unwrap();
        let queue = CommandQueue::new(pistack_rpc::QueueConfig::default());
        let side = DoorSide {
            store: &store,
            queue: Some(&queue),
        };
        let id = "lsn-0123456789abcdef";
        let got = apply(id, DoorKind::Wireguard, "51820", "http://10.1.2.3", side)
            .await
            .unwrap();
        assert_eq!(got.argv[1], "pw0123456789ab");
        assert!(got.argv.iter().all(|arg| !arg.contains("wg-fleet")));
        assert!(!got.argv.iter().any(|arg| arg == "del" || arg == "delete"));
        let wg_text = std::fs::read_to_string(&wg_log).unwrap();
        assert!(wg_text.contains("pw0123456789ab"), "{wg_text}");
        assert!(!wg_text.contains("wg-fleet"), "{wg_text}");
        let ip_text = std::fs::read_to_string(&ip_log).unwrap();
        assert_eq!(
            ip_text.lines().collect::<Vec<_>>(),
            vec!["link", "add", "dev", "pw0123456789ab", "type", "wireguard"]
        );
        assert!(!ip_text.contains("wg-fleet"));
        assert!(!ip_text.contains("del"));
        apply(id, DoorKind::Wireguard, "51820", "http://10.1.2.3", side)
            .await
            .unwrap();
        let again = std::fs::read_to_string(&ip_log).unwrap();
        assert_eq!(again, ip_text, "wireguard create ran twice");

        let no_edge = apply(
            "edge-none",
            DoorKind::Edge,
            "80",
            "http://127.0.0.1:9",
            side,
        )
        .await
        .unwrap_err();
        assert!(no_edge.message.contains("no edge host"));
        assert_eq!(queue.pending_count(), 0);

        host(&store, "kvm-1", &["libvirt"]);
        let still = apply("edge-kvm", DoorKind::Edge, "80", "http://127.0.0.1:9", side)
            .await
            .unwrap_err();
        assert!(still.message.contains("no edge host"));
        assert_eq!(queue.pending_count(), 0);

        host(&store, "b-edge", &["edge"]);
        host(&store, "a-edge", &["kvm", "edge"]);
        let published = apply(
            "edge-ok",
            DoorKind::Edge,
            "443",
            "http://10.1.0.2:8080",
            side,
        )
        .await
        .unwrap();
        assert!(published.bin.is_empty());
        assert!(published.note.contains("a-edge"));
        apply(
            "edge-ok",
            DoorKind::Edge,
            "443",
            "http://10.1.0.2:8080",
            side,
        )
        .await
        .unwrap();
        assert_eq!(queue.pending_count(), 1);
        let cmds = queue
            .next("a-edge", std::time::Duration::from_millis(200), 4)
            .await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind, CommandKind::Publish);
        assert_ne!(cmds[0].kind, CommandKind::StopMachine);
        assert_ne!(cmds[0].kind, CommandKind::DeleteMachine);
        let body = cmds[0].payload.to_string();
        assert!(body.contains("edge-ok"), "{body}");
        assert!(!body.contains("ssh"), "{body}");
        assert!(!body.contains("wg-fleet"), "{body}");
        assert!(queue
            .next("b-edge", std::time::Duration::from_millis(50), 4)
            .await
            .is_empty());
        stop(id);
        stop("edge-ok");

        match _wg {
            Some(v) => std::env::set_var("PISTACK_WG", v),
            None => std::env::remove_var("PISTACK_WG"),
        }
        match _ip {
            Some(v) => std::env::set_var("PISTACK_IP", v),
            None => std::env::remove_var("PISTACK_IP"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
