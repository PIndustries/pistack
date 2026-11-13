// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi host` against a small in-process server that keeps one host and
//! records what the CLI sent.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post, put},
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

#[derive(Clone)]
struct Mock {
    host: Arc<Mutex<Value>>,
    /// `(path, body)` of every write, in order.
    writes: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Mock {
    fn last_write(&self) -> (String, Value) {
        self.writes
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a write")
    }
}

async fn show(State(m): State<Mock>, Path(id): Path<String>) -> axum::response::Response {
    let host = m.host.lock().unwrap().clone();
    if id != "rack1" && host["id"] != id.as_str() {
        return (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"code": "not_found", "message": "no such host", "details": {}}})),
        )
            .into_response();
    }
    Json(host).into_response()
}

async fn list(State(m): State<Mock>) -> Json<Value> {
    Json(json!({"items": [m.host.lock().unwrap().clone()], "total": 1}))
}

async fn set_labels(
    State(m): State<Mock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    m.writes
        .lock()
        .unwrap()
        .push((format!("/compute/hosts/{id}/labels"), body.clone()));
    let mut host = m.host.lock().unwrap();
    // Like the server: the caller's set replaces everything but pistack.io/.
    let mut labels = body["labels"].clone();
    labels["pistack.io/agent"] = json!("true");
    host["labels"] = labels;
    Json(host.clone())
}

async fn disable(
    State(m): State<Mock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    m.writes
        .lock()
        .unwrap()
        .push((format!("/compute/hosts/{id}/disable"), body.clone()));
    let mut host = m.host.lock().unwrap();
    host["spec"]["disabled"] = json!(true);
    if let Some(reason) = body.get("reason") {
        host["spec"]["disabled_reason"] = reason.clone();
    }
    Json(host.clone())
}

async fn enable(State(m): State<Mock>, Path(id): Path<String>) -> Json<Value> {
    m.writes
        .lock()
        .unwrap()
        .push((format!("/compute/hosts/{id}/enable"), Value::Null));
    let mut host = m.host.lock().unwrap();
    host["spec"]["disabled"] = json!(false);
    host["spec"]
        .as_object_mut()
        .unwrap()
        .remove("disabled_reason");
    Json(host.clone())
}

async fn drain(
    State(m): State<Mock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> Json<Value> {
    let mut writes = m.writes.lock().unwrap();
    let earlier = writes.iter().filter(|(p, _)| p.ends_with("/drain")).count();
    writes.push((format!("/compute/hosts/{id}/drain"), body));
    let host = m.host.lock().unwrap().clone();
    // The first call starts a migration; the second finds the host empty.
    if earlier > 0 {
        return Json(json!({"host": host, "migrating": [], "not_moved": []}));
    }
    Json(json!({
        "host": host,
        "migrating": [{
            "id": "mach-0199a5e0-aaaa-7000-8000-000000000002",
            "name": "web",
            "to": "host-0199a5e0-aaaa-7000-8000-000000000009",
            "to_name": "rack2",
        }],
        "not_moved": [{
            "id": "mach-0199a5e0-aaaa-7000-8000-000000000003",
            "name": "pod",
            "reason": "is a container; only a virtual machine can be live-migrated",
        }],
    }))
}

async fn evacuate(
    State(m): State<Mock>,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    m.writes
        .lock()
        .unwrap()
        .push((format!("/compute/hosts/{id}/evacuate"), body.clone()));
    let host = m.host.lock().unwrap().clone();
    if host["spec"]["disabled"] != json!(true) {
        return (
            StatusCode::CONFLICT,
            Json(json!({"error": {"code": "conflict", "message": "host rack1 is up", "details": {}}})),
        )
            .into_response();
    }
    let pod = json!({
        "id": "mach-0199a5e0-aaaa-7000-8000-000000000003",
        "name": "db",
        "reason": "volume scratch is on local storage on this host and cannot follow the machine",
    });
    if body["rebuild_from_image"] != json!(true) {
        let web = json!({
            "id": "mach-0199a5e0-aaaa-7000-8000-000000000002",
            "name": "web",
            "reason": "its root disk is on this host",
        });
        return Json(json!({"host": host, "evacuated": [], "skipped": [web, pod]})).into_response();
    }
    Json(json!({
        "host": host,
        "evacuated": [{
            "id": "mach-0199a5e0-aaaa-7000-8000-000000000002",
            "name": "web",
            "rebuilt_from": "ubuntu-24.04",
            "volumes": ["blocks"],
        }],
        "skipped": [pod],
    }))
    .into_response()
}

async fn create_machine(
    State(m): State<Mock>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    m.writes
        .lock()
        .unwrap()
        .push(("/compute/machines".into(), body.clone()));
    let v = json!({
        "id": "mach-0199a5e0-aaaa-7000-8000-000000000002",
        "name": body["name"],
        "spec": body["spec"],
        "status": {"phase": "pending"},
    });
    (StatusCode::CREATED, Json(v)).into_response()
}

async fn serve() -> (String, Mock) {
    let mock = Mock {
        host: Arc::new(Mutex::new(json!({
            "id": "host-0199a5e0-aaaa-7000-8000-000000000001",
            "name": "rack1",
            "labels": {"pistack.io/agent": "true", "rack": "r1"},
            "spec": {"node_name": "rack1", "drivers": ["kvm", "libvirt"]},
            "status": {"phase": "ready"},
        }))),
        writes: Default::default(),
    };
    let app = Router::new()
        .route("/compute/machines", post(create_machine))
        .route("/compute/hosts", get(list))
        .route("/compute/hosts/{id}", get(show))
        .route("/compute/hosts/{id}/labels", put(set_labels))
        .route("/compute/hosts/{id}/disable", post(disable))
        .route("/compute/hosts/{id}/enable", post(enable))
        .route("/compute/hosts/{id}/drain", post(drain))
        .route("/compute/hosts/{id}/evacuate", post(evacuate))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), mock)
}

struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(url: &str, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let mut argv: Vec<String> = [
        "pi",
        "--config",
        dir.path().join("config.toml").to_str().unwrap(),
        "--url",
        url,
        "--token",
        "t",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.extend(args.iter().map(|s| s.to_string()));
    let mut input = Cursor::new(Vec::new());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut io = Io {
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut err,
            tty: false,
        };
        cli::run(argv, &mut io).await.expect("run")
    };
    Run {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

#[tokio::test]
async fn host_label_merges_removes_and_replaces() {
    let (url, mock) = serve().await;

    // Setting one label keeps the others and never sends a pistack.io/ one.
    let r = pi(&url, &["host", "label", "rack1", "zone=west"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("host \"rack1\" labelled"), "{}", r.out);
    let (path, body) = mock.last_write();
    assert_eq!(path, "/compute/hosts/rack1/labels");
    assert_eq!(body, json!({"labels": {"rack": "r1", "zone": "west"}}));

    // The zone shows in the list.
    let r = pi(&url, &["host", "list"]).await;
    assert!(r.out.contains("ZONE"), "{}", r.out);
    assert!(r.out.contains("west"), "{}", r.out);

    let r = pi(&url, &["host", "label", "rack1", "--remove", "rack"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(mock.last_write().1, json!({"labels": {"zone": "west"}}));

    let r = pi(&url, &["host", "label", "rack1", "--replace", "tier=gold"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(mock.last_write().1, json!({"labels": {"tier": "gold"}}));

    let r = pi(&url, &["host", "label", "rack1", "zone"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("key=value"), "{}", r.err);
}

#[tokio::test]
async fn machine_create_sends_the_zone() {
    let (url, mock) = serve().await;
    let r = pi(
        &url,
        &[
            "machine", "create", "web", "--size", "s.1x1", "--image", "ubuntu", "--zone", "west",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(mock.last_write().1["spec"]["zone"], "west");

    // Without the flag the field is left out, not sent empty.
    let r = pi(
        &url,
        &[
            "machine", "create", "db", "--size", "s.1x1", "--image", "ubuntu",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(mock.last_write().1["spec"].get("zone").is_none());
}

#[tokio::test]
async fn host_disable_shows_the_reason_and_enable_clears_it() {
    let (url, mock) = serve().await;

    let r = pi(&url, &["host", "list"]).await;
    assert!(r.out.contains("SCHEDULING"), "{}", r.out);
    assert!(r.out.contains("enabled"), "{}", r.out);

    let r = pi(
        &url,
        &["host", "disable", "rack1", "--reason", "replacing a fan"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("host \"rack1\" disabled"), "{}", r.out);
    let (path, body) = mock.last_write();
    assert_eq!(path, "/compute/hosts/rack1/disable");
    assert_eq!(body, json!({"reason": "replacing a fan"}));

    let r = pi(&url, &["host", "show", "rack1"]).await;
    assert!(r.out.contains("replacing a fan"), "{}", r.out);
    let r = pi(&url, &["host", "list"]).await;
    assert!(r.out.contains("disabled (replacing a fan)"), "{}", r.out);

    // No reason sends no reason.
    let r = pi(&url, &["host", "disable", "rack1"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(mock.last_write().1, json!({}));

    let r = pi(&url, &["host", "enable", "rack1"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("host \"rack1\" enabled"), "{}", r.out);
    assert_eq!(mock.last_write().0, "/compute/hosts/rack1/enable");
    let r = pi(&url, &["host", "list"]).await;
    assert!(!r.out.contains("disabled"), "{}", r.out);
}

#[tokio::test]
async fn host_drain_lists_what_moves_and_what_stays() {
    let (url, mock) = serve().await;

    let r = pi(
        &url,
        &["host", "drain", "rack1", "--reason", "kernel update"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let (path, body) = mock.last_write();
    assert_eq!(path, "/compute/hosts/rack1/drain");
    assert_eq!(body, json!({"reason": "kernel update"}));
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(
        lines,
        [
            "host \"rack1\" disabled",
            "machine \"web\" migrating to rack2",
            "machine \"pod\" not moved: is a container; only a virtual machine can be live-migrated",
            "2 still on host \"rack1\": 1 migrating, 1 not moved. Run the command again to check.",
        ]
    );

    let r = pi(&url, &["host", "drain", "rack1"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out.contains("no machines are left on host \"rack1\""),
        "{}",
        r.out
    );

    // Other output formats get the whole report.
    let r = pi(&url, &["-o", "json", "host", "drain", "rack1"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert!(v["migrating"].is_array(), "{}", r.out);
}

#[tokio::test]
async fn host_evacuate_is_loud_about_lost_disks() {
    let (url, mock) = serve().await;

    // The server's refusal for a host that is up is shown as it is.
    let r = pi(&url, &["host", "evacuate", "rack1", "--rebuild-from-image"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("host rack1 is up"), "{}", r.err);

    let r = pi(&url, &["host", "disable", "rack1"]).await;
    assert_eq!(r.code, 0, "{}", r.err);

    // Without the flag nothing is evacuated and nothing is warned about.
    let r = pi(&url, &["host", "evacuate", "rack1", "--confirm-dead"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        mock.last_write().1,
        json!({"confirm_dead": true, "rebuild_from_image": false})
    );
    assert!(!r.out.contains("WARNING"), "{}", r.out);
    assert!(
        r.out
            .contains("machine \"web\" skipped: its root disk is on this host"),
        "{}",
        r.out
    );
    assert!(r.out.contains("0 evacuated, 2 skipped"), "{}", r.out);

    let r = pi(
        &url,
        &[
            "host",
            "evacuate",
            "rack1",
            "--confirm-dead",
            "--rebuild-from-image",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        mock.last_write().1,
        json!({"confirm_dead": true, "rebuild_from_image": true})
    );
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines.len(), 4, "{}", r.out);
    assert!(
        lines[0].starts_with("WARNING: ROOT DISKS ARE LOST."),
        "{}",
        r.out
    );
    assert_eq!(
        lines[1],
        "machine \"web\" evacuated: root disk lost, starts from image ubuntu-24.04; volumes plugged back in when it runs: blocks"
    );
    assert_eq!(
        lines[2],
        "machine \"db\" skipped: volume scratch is on local storage on this host and cannot follow the machine"
    );
    assert_eq!(lines[3], "1 evacuated, 1 skipped");
}
