// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi backup` against a small in-process server that speaks the block
//! storage service's backup routes: one volume, one bucket, and backups kept
//! in a map. Every write the CLI makes is recorded so a test can read the
//! body back.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const VOL_ID: &str = "vol-0199a5e0-aaaa-7000-8000-000000000001";
const BAK_ID: &str = "bak-0199a5e0-aaaa-7000-8000-00000000000a";
const BAK_ID2: &str = "bak-0199a5e0-aaaa-7000-8000-00000000000b";

#[derive(Clone)]
struct Mock {
    backups: Arc<Mutex<BTreeMap<String, Value>>>,
    /// `(path, body)` of every write, in order.
    writes: Arc<Mutex<Vec<(String, Value)>>>,
    /// When set, the next look reports the upload failed.
    fail_next: Arc<Mutex<bool>>,
    /// When set, a `GET` of a backup answers not found, as it does once the
    /// object is gone.
    vanish: Arc<Mutex<bool>>,
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

/// Backups are keyed by id, as the service keys everything. A `GET` or a
/// `DELETE` also accepts a name, so the key is found either way.
fn key_of(backups: &BTreeMap<String, Value>, name_or_id: &str) -> Option<String> {
    backups
        .keys()
        .find(|k| **k == name_or_id || backups[*k]["name"] == json!(name_or_id))
        .cloned()
}

fn not_found(what: &str) -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": {"code": "not_found", "message": format!("{what} not found"), "details": {}}})),
    )
        .into_response()
}

fn volume(id: &str, name: &str, size_gib: u64) -> Value {
    json!({
        "id": id,
        "name": name,
        "spec": {"size_gib": size_gib, "volume_type": "standard", "source": {"kind": "none"}},
        "status": {"phase": "ready"},
    })
}

/// Volume by name or id, so the CLI's name lookup has somewhere to land.
async fn show_volume(Path(id): Path<String>) -> axum::response::Response {
    if id == "data" || id == VOL_ID {
        return Json(volume(VOL_ID, "data", 4)).into_response();
    }
    not_found(&format!("volume '{id}'"))
}

async fn list(State(m): State<Mock>) -> Json<Value> {
    let items: Vec<Value> = m.backups.lock().unwrap().values().cloned().collect();
    let total = items.len();
    Json(json!({"items": items, "total": total}))
}

async fn show(State(m): State<Mock>, Path(id): Path<String>) -> axum::response::Response {
    let mut backups = m.backups.lock().unwrap();
    let key = match key_of(&backups, &id) {
        Some(k) => k,
        None => return not_found(&format!("backup '{id}'")),
    };
    if *m.vanish.lock().unwrap() {
        backups.remove(&key);
        return not_found(&format!("backup '{id}'"));
    }
    let b = backups.get_mut(&key).unwrap();
    // Like the service: a failed upload says so once, in its conditions.
    if *m.fail_next.lock().unwrap() {
        *m.fail_next.lock().unwrap() = false;
        b["status"]["phase"] = json!("error");
        b["status"]["conditions"] = json!([{
            "type": "Uploaded",
            "status": false,
            "reason": "not_found",
            "message": "bucket 'cold' not found",
            "last_transition": "2026-11-14T00:00:00Z",
        }]);
        return Json(b.clone()).into_response();
    }
    // An upload in progress moves on each look.
    if b["status"]["phase"] == json!("pending") {
        b["status"]["phase"] = json!("creating");
    } else if b["status"]["phase"] == json!("creating") {
        b["status"]["phase"] = json!("ready");
        b["status"]["object_ref"] = json!("backups/nightly");
        b["status"]["part_count"] = json!(2);
    }
    Json(b.clone()).into_response()
}

async fn create(State(m): State<Mock>, Json(body): Json<Value>) -> axum::response::Response {
    m.writes
        .lock()
        .unwrap()
        .push(("/volume/backups".into(), body.clone()));
    if body["spec"]["target"].as_str().unwrap_or("") == "missing" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "error": {"code": "bad_request", "message": "spec.target (bucket) is required", "details": {}}
            })),
        )
            .into_response();
    }
    let name = body["name"].as_str().unwrap_or("").to_owned();
    // One id per name, the way the service hands out one id per backup.
    let id = if name == "nightly" { BAK_ID } else { BAK_ID2 };
    let b = json!({
        "id": id,
        "name": name,
        "description": body["description"],
        "labels": body["labels"],
        "project": "proj-0199a5e0-2222-7000-8000-000000000002",
        "created_at": "2026-11-14T00:00:00Z",
        "spec": body["spec"],
        "status": {"phase": "pending", "size_gib": 4},
    });
    m.backups.lock().unwrap().insert(id.to_owned(), b.clone());
    (StatusCode::CREATED, Json(b)).into_response()
}

async fn delete(State(m): State<Mock>, Path(id): Path<String>) -> axum::response::Response {
    let mut backups = m.backups.lock().unwrap();
    let key = match key_of(&backups, &id) {
        Some(k) => k,
        None => return not_found(&format!("backup '{id}'")),
    };
    m.writes
        .lock()
        .unwrap()
        .push((format!("/volume/backups/{key}"), Value::Null));
    backups.get_mut(&key).unwrap()["status"]["phase"] = json!("deleting");
    StatusCode::ACCEPTED.into_response()
}

async fn restore(
    State(m): State<Mock>,
    Path(id): Path<String>,
    body: String,
) -> axum::response::Response {
    let sent: Value = if body.trim().is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&body).unwrap_or(Value::Null)
    };
    let found = {
        let backups = m.backups.lock().unwrap();
        key_of(&backups, &id).and_then(|k| backups.get(&k)).cloned()
    };
    let Some(backup) = found else {
        return not_found(&format!("backup '{id}'"));
    };
    m.writes
        .lock()
        .unwrap()
        .push((format!("/volume/backups/{id}/restore"), sent.clone()));
    if backup["status"]["phase"] != json!("ready") {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "error": {"code": "conflict", "message": format!("backup {id} is {:?}, not ready", backup["status"]["phase"]), "details": {}}
            })),
        )
            .into_response();
    }
    // An existing volume is written into; an empty body makes a new one
    // named after the backup.
    let (id, name, size) = match sent["volume"].as_str() {
        Some("data") | Some(VOL_ID) => (VOL_ID.to_owned(), "data".to_owned(), 4),
        Some(other) => return not_found(&format!("volume '{other}'")),
        None => (
            "vol-0199a5e0-aaaa-7000-8000-00000000000c".to_owned(),
            format!("restore-{}", backup["id"].as_str().unwrap_or(BAK_ID)),
            4,
        ),
    };
    Json(volume(&id, &name, size)).into_response()
}

async fn serve() -> (String, Mock) {
    let mock = Mock {
        backups: Arc::new(Mutex::new(BTreeMap::new())),
        writes: Default::default(),
        fail_next: Default::default(),
        vanish: Default::default(),
    };
    let ready = json!({
        "id": BAK_ID2,
        "name": "weekly",
        "description": "week 46",
        "labels": {"tier": "gold"},
        "project": "proj-0199a5e0-2222-7000-8000-000000000002",
        "created_at": "2026-11-14T00:00:00Z",
        "spec": {"volume": VOL_ID, "target": "cold"},
        "status": {
            "phase": "ready",
            "size_gib": 4,
            "size_bytes": 4294967296u64,
            "part_count": 3,
            "sha256": "aa".repeat(32),
            "object_ref": "backups/weekly",
        },
    });
    mock.backups
        .lock()
        .unwrap()
        .insert(BAK_ID2.to_owned(), ready);
    let app = Router::new()
        .route("/volume/volumes/{id}", get(show_volume))
        .route("/volume/backups", get(list).post(create))
        .route("/volume/backups/{id}", get(show).delete(delete))
        .route("/volume/backups/{id}/restore", post(restore))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), mock)
}

#[derive(Debug)]
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
async fn backup_list_shows_the_columns() {
    let (url, _m) = serve().await;

    let r = pi(&url, &["backup", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("NAME"), "{}", r.out);
    assert!(r.out.contains("PHASE"), "{}", r.out);
    assert!(r.out.contains("BUCKET"), "{}", r.out);
    assert!(r.out.contains("SIZE-GIB"), "{}", r.out);
    assert!(r.out.contains("weekly"), "{}", r.out);
    // A short volume id, never the whole thing.
    assert!(r.out.contains("vol-00000001"), "{}", r.out);
    assert!(!r.out.contains(VOL_ID), "{}", r.out);

    let r = pi(&url, &["backup", "ls", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("OBJECT"), "{}", r.out);
    assert!(r.out.contains("backups/weekly"), "{}", r.out);

    let r = pi(&url, &["backup", "list", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let listed: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0]["name"], "weekly");

    let r = pi(&url, &["backup", "list", "-o", "yaml"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("name: weekly"), "{}", r.out);

    let r = pi(&url, &["backup", "list", "-o", "name"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "weekly");
}

#[tokio::test]
async fn backup_show_accepts_a_name_or_an_id() {
    let (url, _m) = serve().await;

    let r = pi(&url, &["backup", "show", "weekly", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["id"], BAK_ID2);
    assert_eq!(v["status"]["phase"], "ready");

    let r = pi(&url, &["backup", "get", BAK_ID2, "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains(BAK_ID2), "{}", r.out);

    // The table form is a FIELD/VALUE dump of the one resource.
    let r = pi(&url, &["backup", "show", "weekly"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("phase"), "{}", r.out);

    let r = pi(&url, &["backup", "show", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("backup 'nope' not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_create_resolves_the_volume_and_names_the_bucket() {
    let (url, mock) = serve().await;

    let r = pi(
        &url,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("backup \"nightly\" created"), "{}", r.out);
    let (path, body) = mock.last_write();
    assert_eq!(path, "/volume/backups");
    assert_eq!(body["name"], "nightly");
    // The volume was looked up by name and sent as an id.
    assert_eq!(body["spec"]["volume"], VOL_ID);
    assert_eq!(body["spec"]["target"], "cold");
    assert!(body.get("project").is_none(), "{body}");

    // Metadata goes where every other create puts it.
    let r = pi(
        &url,
        &[
            "backup",
            "create",
            "twice",
            "--volume",
            VOL_ID,
            "--bucket",
            "cold",
            "--description",
            "before the upgrade",
            "--label",
            "tier=gold",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let (_, body) = mock.last_write();
    assert_eq!(body["description"], "before the upgrade");
    assert_eq!(body["labels"], json!({"tier": "gold"}));
    assert_eq!(body["spec"]["volume"], VOL_ID);

    let r = pi(
        &url,
        &[
            "backup", "create", "x", "--volume", "ghost", "--bucket", "cold",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("volume 'ghost' not found"), "{}", r.err);

    // The server's own refusal is reported as the server words it.
    let r = pi(
        &url,
        &[
            "backup", "create", "x", "--volume", "data", "--bucket", "missing",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("spec.target"), "{}", r.err);
}

#[tokio::test]
async fn backup_create_wait_polls_until_ready() {
    let (url, _m) = serve().await;

    let r = pi(
        &url,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold", "--wait",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("backup \"nightly\" created"), "{}", r.out);
    // The phase the create came back with is not repeated; each change after
    // it is printed once, and ready ends the wait.
    assert!(!r.out.contains(": pending"), "{}", r.out);
    assert!(r.out.contains("backup \"nightly\": creating"), "{}", r.out);
    assert!(r.out.contains("backup \"nightly\": ready"), "{}", r.out);
    assert_eq!(
        r.out.matches("backup \"nightly\": ready").count(),
        1,
        "{r:?}"
    );
}

#[tokio::test]
async fn backup_create_wait_reports_a_failed_upload() {
    let (url, mock) = serve().await;
    let r = pi(
        &url,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    // The upload fails now, so the next look says so.
    *mock.fail_next.lock().unwrap() = true;

    let r = pi(
        &url,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold", "--wait",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("backup \"nightly\" failed"), "{}", r.err);
    // The reason comes from the condition the service records.
    assert!(r.err.contains("bucket 'cold' not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_delete_needs_yes_and_reports_acceptance() {
    let (url, mock) = serve().await;

    // No terminal to confirm on, so a delete without --yes is refused.
    let r = pi(&url, &["backup", "delete", "weekly"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert!(mock.writes.lock().unwrap().is_empty());

    let r = pi(&url, &["backup", "delete", "weekly", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    // The service answers 202: the object goes away in the background.
    assert!(
        r.out.contains("backup \"weekly\" deletion requested"),
        "{}",
        r.out
    );
    assert_eq!(mock.last_write().0, format!("/volume/backups/{BAK_ID2}"));

    let r = pi(&url, &["backup", "rm", "ghost", "--yes"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("backup 'ghost' not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_delete_wait_polls_until_gone() {
    let (url, mock) = serve().await;
    // The stored object is gone by the time the delete is confirmed.
    *mock.vanish.lock().unwrap() = true;

    let r = pi(&url, &["backup", "delete", "weekly", "--yes", "--wait"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.out.contains("backup \"weekly\" deletion requested"),
        "{}",
        r.out
    );
    assert!(r.out.contains("backup \"weekly\": gone"), "{}", r.out);
}

#[tokio::test]
async fn backup_restore_makes_a_new_volume_or_writes_the_one_named() {
    let (url, mock) = serve().await;

    let r = pi(&url, &["backup", "restore", "weekly"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("restored"), "{}", r.out);
    let (path, body) = mock.last_write();
    assert_eq!(path, "/volume/backups/weekly/restore");
    // A new volume is asked for with a body that names no volume.
    assert_eq!(body, json!({}));

    let r = pi(&url, &["backup", "restore", BAK_ID2, "--volume", "data"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("volume \"data\" restored"), "{}", r.out);
    let (path, body) = mock.last_write();
    assert_eq!(path, format!("/volume/backups/{BAK_ID2}/restore"));
    assert_eq!(body["volume"], VOL_ID);

    let r = pi(&url, &["backup", "restore", "weekly", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["name"], format!("restore-{BAK_ID2}"));

    let r = pi(&url, &["backup", "restore", "weekly", "--volume", "ghost"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("volume 'ghost' not found"), "{}", r.err);

    let r = pi(&url, &["backup", "restore", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("backup 'nope' not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_restore_of_a_pending_backup_is_refused() {
    let (url, _m) = serve().await;

    // Create one and leave it pending: the bytes are not stored yet.
    let r = pi(
        &url,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");

    let r = pi(&url, &["backup", "restore", "nightly"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not ready"), "{}", r.err);
}

#[tokio::test]
async fn backup_noun_is_not_mixed_into_other_verbs() {
    let (url, _m) = serve().await;

    let r = pi(&url, &["backup", "attach", "weekly"]).await;
    assert_eq!(r.code, 2, "{r:?}");
}
