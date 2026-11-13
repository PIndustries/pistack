// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi host` against a small in-process server that keeps one host and
//! records what the CLI sent.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, put},
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
        .route("/compute/hosts", get(list))
        .route("/compute/hosts/{id}", get(show))
        .route("/compute/hosts/{id}/labels", put(set_labels))
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
