// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Create, list and reject a bad protocol against a temp on-disk store.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use shelf::Shelf;
use std::sync::{Arc, OnceLock};
use tower::ServiceExt;

fn shelf_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

fn install_share_tools(dir: &std::path::Path) {
    let script = dir.join("exportfs");
    std::fs::write(&script, "#!/bin/sh\nexit 0\n").expect("exportfs script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&script).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&script, perms).unwrap();
    }
    std::env::set_var("PISTACK_EXPORTFS", &script);
    std::env::set_var("PISTACK_CEPH_BIN", dir.join("no-ceph"));
    std::env::remove_var("PISTACK_CEPH_FS");
}

fn clear_share_tools() {
    std::env::remove_var("PISTACK_EXPORTFS");
    std::env::remove_var("PISTACK_CEPH_BIN");
    std::env::remove_var("PISTACK_CEPH_FS");
    std::env::remove_var("PISTACK_SHELF_DIR");
}

fn open_store(dir: &std::path::Path) -> Store {
    let (events, _rx) = tokio::sync::broadcast::channel(32);
    Store::open(dir.join("state.redb"), events).expect("open store")
}

fn router(store: Store) -> axum::Router {
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: pistack_types::module::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
    };
    Shelf.router(ctx)
}

async fn call(
    app: &axum::Router,
    method: &str,
    uri: &str,
    body: Option<Value>,
    caller: Option<Caller>,
    if_match: Option<&str>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    if body.is_some() {
        req = req.header(header::CONTENT_TYPE, "application/json");
    }
    if let Some(rev) = if_match {
        req = req.header(header::IF_MATCH, rev);
    }
    if let Some(c) = caller {
        req = req.extension(c);
    }
    let res = app
        .clone()
        .oneshot(
            req.body(match body {
                Some(v) => Body::from(v.to_string()),
                None => Body::empty(),
            })
            .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes)
            .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into()))
    };
    (status, v)
}

fn admin() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

fn member(project: &Id) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "alice".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    }
}

#[tokio::test]
async fn create_list_and_reject_bad_protocol() {
    let _guard = shelf_lock().lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let shelf = dir.path().join("shares");
    std::env::set_var("PISTACK_SHELF_DIR", &shelf);
    install_share_tools(dir.path());
    let app = router(open_store(dir.path()));
    let info = Shelf.info();
    assert_eq!(info.name, "shelf");
    assert_eq!(info.prefix, "shelf");

    let (st, err) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "bad",
            "spec": { "protocol": "smb", "size_gib": 10 }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{err}");
    assert_eq!(err["error"]["code"], "bad_request");

    let (st, err) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "zero",
            "spec": { "protocol": "nfs", "size_gib": 0 }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{err}");

    let (st, err) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "nobody",
            "spec": { "protocol": "nfs", "size_gib": 1, "access": [{ "mode": "read" }] }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "{err}");

    let (st, list) = call(&app, "GET", "/shares", None, Some(admin()), None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 0);

    let (st, share) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "home",
            "spec": {
                "protocol": "nfs",
                "size_gib": 10,
                "access": [{ "cidr": "10.0.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    let id = share["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("share-"), "{id}");
    assert_eq!(share["name"], "home");
    assert_eq!(share["spec"]["name"], "home");
    assert_eq!(share["spec"]["protocol"], "nfs");
    assert_eq!(share["spec"]["size_gib"], 10);
    assert_eq!(share["spec"]["access"][0]["mode"], "read");
    assert_eq!(share["status"]["phase"], "ready");
    let home_dir = std::path::PathBuf::from(share["status"]["message"].as_str().unwrap());
    assert!(home_dir.is_dir(), "{}", home_dir.display());
    assert_eq!(
        home_dir.file_name().and_then(|s| s.to_str()),
        Some(id.as_str())
    );
    let exports = std::fs::read_to_string(
        home_dir
            .parent()
            .unwrap()
            .join("exports.d")
            .join(format!("{id}.exports")),
    )
    .unwrap();
    assert_eq!(
        exports,
        format!(
            "{} 10.0.0.0/24(ro,sync,no_subtree_check)\n",
            home_dir.display()
        )
    );
    assert_eq!(share["revision"], 1);
    assert!(share.get("project").is_none());

    let (st, list) = call(&app, "GET", "/shares", None, Some(admin()), None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["id"], id);

    let project = Id::new(kind::PROJECT);
    let alice = member(&project);
    let (st, owned) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "spec": {
                "name": "proj-home",
                "protocol": "cephfs",
                "size_gib": 4,
                "access": [{ "principal": "alice", "mode": "write" }]
            }
        })),
        Some(alice.clone()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{owned}");
    assert_eq!(owned["project"], project.to_string());
    assert_eq!(owned["spec"]["protocol"], "cephfs");
    let owned_id = owned["id"].as_str().unwrap();

    let (st, list) = call(&app, "GET", "/shares", None, Some(alice.clone()), None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 1);
    assert_eq!(list["items"][0]["name"], "proj-home");

    let (st, list) = call(&app, "GET", "/shares", None, Some(admin()), None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 2);

    let other = member(&Id::new(kind::PROJECT));
    let (st, _) = call(
        &app,
        "GET",
        &format!("/shares/{id}"),
        None,
        Some(other),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    let (st, updated) = call(
        &app,
        "PUT",
        &format!("/shares/{owned_id}"),
        Some(json!({
            "spec": {
                "protocol": "nfs",
                "size_gib": 8,
                "access": [{ "principal": "alice", "mode": "read" }]
            }
        })),
        Some(alice.clone()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{updated}");
    assert_eq!(updated["revision"], 2);
    assert_eq!(updated["name"], "proj-home");
    assert_eq!(updated["spec"]["protocol"], "nfs");
    assert_eq!(updated["spec"]["size_gib"], 8);
    assert_eq!(updated["status"]["phase"], "pending");
    assert_eq!(updated["status"]["message"], "nfs share has no client cidr");

    let (st, conflict) = call(
        &app,
        "PUT",
        &format!("/shares/{owned_id}"),
        Some(json!({
            "spec": { "protocol": "cephfs", "size_gib": 9 }
        })),
        Some(alice.clone()),
        Some("1"),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT, "{conflict}");

    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/shares/{owned_id}"),
        None,
        Some(alice.clone()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, list) = call(&app, "GET", "/shares", None, Some(alice), None).await;
    assert_eq!(st, StatusCode::OK, "{list}");
    assert_eq!(list["total"], 0);
    clear_share_tools();
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = router(open_store(dir.path()));
    let (st, body) = call(&app, "GET", "/shares", None, None, None).await;
    assert_eq!(st, StatusCode::UNAUTHORIZED, "{body}");
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}

#[tokio::test]
async fn nfs_share_writes_exports_and_delete_removes_them() {
    let _guard = shelf_lock().lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let shelf = dir.path().join("shares");
    std::env::set_var("PISTACK_SHELF_DIR", &shelf);
    install_share_tools(dir.path());
    let app = router(open_store(dir.path()));

    let (st, ceph) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "ceph",
            "spec": {
                "protocol": "cephfs",
                "size_gib": 2,
                "access": [{ "cidr": "10.9.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{ceph}");
    assert_eq!(ceph["status"]["phase"], "pending");
    assert_eq!(ceph["status"]["message"], "ceph is not installed");

    let (st, quiet) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "quiet",
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "principal": "alice", "mode": "write" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{quiet}");
    assert_eq!(quiet["status"]["phase"], "pending");
    assert_eq!(quiet["status"]["message"], "nfs share has no client cidr");

    let (st, share) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "export",
            "spec": {
                "protocol": "nfs",
                "size_gib": 4,
                "access": [
                    { "cidr": "10.0.0.0/24", "mode": "read" },
                    { "cidr": "10.1.0.0/24", "mode": "write" }
                ]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    assert_eq!(share["status"]["phase"], "ready");
    let id = share["id"].as_str().unwrap();
    let share_dir = std::path::PathBuf::from(share["status"]["message"].as_str().unwrap());
    assert!(share_dir.is_dir(), "{}", share_dir.display());
    assert_eq!(share_dir.file_name().and_then(|s| s.to_str()), Some(id));
    let exports_path = share_dir
        .parent()
        .unwrap()
        .join("exports.d")
        .join(format!("{id}.exports"));
    let exports = std::fs::read_to_string(&exports_path).unwrap();
    let shown = share_dir.display().to_string();
    assert_eq!(
        exports,
        format!(
            "{shown} 10.0.0.0/24(ro,sync,no_subtree_check)\n{shown} 10.1.0.0/24(rw,sync,no_subtree_check)\n"
        )
    );

    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/shares/{id}"),
        None,
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(!share_dir.exists());
    assert!(!exports_path.exists());
    clear_share_tools();
}
