// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Door tests: the argv each kind plans, the argument refusals, the
//! publisher that is started or queued, and the wireguard name guard.

use super::edge::{HostSpecView, HostStatusView};
use super::iface::is_pw;
use super::kind::{DoorKind, DoorSide};
use super::plan::plan;
use super::{apply, stop};
use pistack_rpc::{CommandKind, CommandQueue};
use pistack_store::Store;
use pistack_types::Resource;
use std::path::PathBuf;

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
async fn env_guard() -> tokio::sync::MutexGuard<'static, ()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    LOCK.lock().await
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
    let _env = env_guard().await;
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
    let _env = env_guard().await;
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
