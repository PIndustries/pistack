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
    std::env::set_var("PISTACK_EXPORTS_FILE", dir.join("pistack.exports"));
    std::env::set_var("PISTACK_SHELF_EUID", "0");
    std::env::remove_var("PISTACK_SHELF_HELPER");
    std::env::set_var("PISTACK_CEPH_BIN", dir.join("no-ceph"));
    std::env::remove_var("PISTACK_CEPH_FS");
}

fn clear_share_tools() {
    for key in [
        "PISTACK_EXPORTFS",
        "PISTACK_CEPH_BIN",
        "PISTACK_CEPH_FS",
        "PISTACK_SHELF_DIR",
        "PISTACK_EXPORTS_FILE",
        "PISTACK_SHELF_EUID",
        "PISTACK_SHELF_HELPER",
        "PISTACK_DATA_DIR",
        "PISTACK_SHELF_BACKOFF_MS",
        "PISTACK_SHELF_RETRY_MS",
        "PISTACK_EXPORTFS_LOG",
        "PISTACK_EXPORT_FAIL",
        "PISTACK_SUDO_LOG",
        "PISTACK_HELPER_LOG",
    ] {
        std::env::remove_var(key);
    }
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
        leadership: pistack_types::Leadership::always(),
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
    let exports = std::fs::read_to_string(dir.path().join("pistack.exports")).unwrap();
    let line = format!(
        "{} 10.0.0.0/24(ro,sync,no_subtree_check)",
        home_dir.display()
    );
    assert!(
        exports.starts_with("# Managed by pistack shelf"),
        "{exports}"
    );
    assert!(exports.contains(&line), "{exports}");
    assert_eq!(
        exports
            .lines()
            .filter(|l| l.contains("(ro,") || l.contains("(rw,"))
            .count(),
        1,
        "{exports}"
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
    let exports_path = dir.path().join("pistack.exports");
    let exports = std::fs::read_to_string(&exports_path).unwrap();
    let shown = share_dir.display().to_string();
    assert!(
        exports.starts_with("# Managed by pistack shelf"),
        "{exports}"
    );
    assert!(
        exports.contains(&format!("{shown} 10.0.0.0/24(ro,sync,no_subtree_check)")),
        "{exports}"
    );
    assert!(
        exports.contains(&format!("{shown} 10.1.0.0/24(rw,sync,no_subtree_check)")),
        "{exports}"
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
    let exports = std::fs::read_to_string(&exports_path).unwrap();
    assert!(!exports.contains(&shown), "{exports}");
    assert!(!exports.contains(id), "{exports}");
    clear_share_tools();
}

fn executable(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    std::fs::write(&path, body).expect("script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = std::fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        std::fs::set_permissions(&path, perms).unwrap();
    }
    path
}

fn export_lines(path: &std::path::Path) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    text.lines()
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_owned)
        .collect()
}

struct RestoreEnv {
    key: &'static str,
    prev: Option<std::ffi::OsString>,
}

impl RestoreEnv {
    fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
        let prev = std::env::var_os(key);
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for RestoreEnv {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

async fn stop_loop(shutdown: pistack_types::module::CancellationToken) {
    shutdown.cancel();
    tokio::task::yield_now().await;
    shelf::quiesce().await;
    tokio::task::yield_now().await;
    shelf::quiesce().await;
}

#[tokio::test]
async fn server_creates_share_root_and_share_directory() {
    let _guard = shelf_lock().lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let bad = dir.path().join("not-a-directory");
    std::fs::write(&bad, "x").unwrap();
    let bad_root = bad.join("shares");
    std::env::set_var("PISTACK_SHELF_DIR", &bad_root);
    std::env::remove_var("PISTACK_DATA_DIR");
    std::env::set_var("PISTACK_EXPORTS_FILE", dir.path().join("pistack.exports"));
    std::env::set_var("PISTACK_SHELF_EUID", "0");
    std::env::remove_var("PISTACK_SHELF_HELPER");
    executable(dir.path(), "exportfs", "#!/bin/sh\nexit 0\n");
    std::env::set_var("PISTACK_EXPORTFS", dir.path().join("exportfs"));
    std::env::set_var("PISTACK_CEPH_BIN", dir.path().join("no-ceph"));
    let app = router(open_store(dir.path()));

    let (st, blocked) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "blocked",
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "cidr": "10.0.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{blocked}");
    assert_eq!(blocked["status"]["phase"], "error");
    let message = blocked["status"]["message"].as_str().unwrap();
    assert!(message.contains("cannot create share root"), "{message}");
    assert!(
        message.contains(&bad_root.display().to_string()),
        "{message}"
    );

    std::env::remove_var("PISTACK_SHELF_DIR");
    let data = dir.path().join("data");
    std::env::set_var("PISTACK_DATA_DIR", &data);
    let root = data.join("shares");
    assert!(!root.exists(), "share root is created by the server");

    let (st, share) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "home",
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "cidr": "10.8.0.0/24", "mode": "write" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    assert_eq!(share["status"]["phase"], "ready", "{share}");
    assert!(root.is_dir(), "{}", root.display());
    let share_dir = std::path::PathBuf::from(share["status"]["message"].as_str().unwrap());
    assert!(share_dir.is_dir(), "{}", share_dir.display());
    assert_eq!(
        share_dir.parent().and_then(|p| p.canonicalize().ok()),
        root.canonicalize().ok()
    );
    let id = share["id"].as_str().unwrap();
    assert_eq!(share_dir.file_name().and_then(|s| s.to_str()), Some(id));
    clear_share_tools();
}

#[tokio::test]
async fn managed_exports_file_adds_updates_and_deletes() {
    let _guard = shelf_lock().lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let shelf = dir.path().join("shares");
    std::env::set_var("PISTACK_SHELF_DIR", &shelf);
    let exports_path = dir.path().join("pistack.exports");
    let log = dir.path().join("exportfs.log");
    install_share_tools(dir.path());
    executable(
        dir.path(),
        "exportfs",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_EXPORTFS_LOG\"\nexit 0\n",
    );
    std::env::set_var("PISTACK_EXPORTFS_LOG", &log);
    let app = router(open_store(dir.path()));

    let (st, first) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "alpha",
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "cidr": "10.0.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{first}");
    assert_eq!(first["status"]["phase"], "ready");
    let first_id = first["id"].as_str().unwrap();
    let first_dir = first["status"]["message"].as_str().unwrap();
    let first_line = format!("{first_dir} 10.0.0.0/24(ro,sync,no_subtree_check)");

    let (st, second) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "beta",
            "spec": {
                "protocol": "nfs",
                "size_gib": 2,
                "access": [{ "cidr": "10.1.0.0/24", "mode": "write" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{second}");
    let second_id = second["id"].as_str().unwrap();
    let second_dir = second["status"]["message"].as_str().unwrap();
    let second_line = format!("{second_dir} 10.1.0.0/24(rw,sync,no_subtree_check)");
    let body = std::fs::read_to_string(&exports_path).unwrap();
    assert!(body.starts_with("# Managed by pistack shelf"), "{body}");
    let lines = export_lines(&exports_path);
    assert!(lines.contains(&first_line), "{lines:?}");
    assert!(lines.contains(&second_line), "{lines:?}");
    assert_eq!(lines.len(), 2, "{lines:?}");
    assert!(!shelf.join("exports.d").exists());

    let (st, updated) = call(
        &app,
        "PUT",
        &format!("/shares/{first_id}"),
        Some(json!({
            "spec": {
                "protocol": "nfs",
                "size_gib": 3,
                "access": [{ "cidr": "10.2.0.0/24", "mode": "write" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{updated}");
    assert_eq!(updated["status"]["phase"], "ready");
    let updated_dir = updated["status"]["message"].as_str().unwrap();
    let updated_line = format!("{updated_dir} 10.2.0.0/24(rw,sync,no_subtree_check)");
    let lines = export_lines(&exports_path);
    assert!(lines.contains(&updated_line), "{lines:?}");
    assert!(lines.contains(&second_line), "{lines:?}");
    assert!(
        !lines.iter().any(|l| l.contains("10.0.0.0/24")),
        "{lines:?}"
    );

    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/shares/{first_id}"),
        None,
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(!std::path::Path::new(first_dir).exists());
    let lines = export_lines(&exports_path);
    assert_eq!(lines, vec![second_line.clone()], "{lines:?}");

    let (st, _) = call(
        &app,
        "DELETE",
        &format!("/shares/{second_id}"),
        None,
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    assert!(export_lines(&exports_path).is_empty());
    let log_text = std::fs::read_to_string(&log).unwrap();
    assert!(log_text.lines().any(|line| line == "-ra"), "{log_text}");
    clear_share_tools();
}

#[tokio::test]
async fn failed_export_is_retried_after_backoff() {
    let _guard = shelf_lock().lock().await;
    let dir = tempfile::tempdir().expect("tempdir");
    let shelf = dir.path().join("shares");
    let log = dir.path().join("exportfs.log");
    let fail = dir.path().join("fail");
    std::fs::write(&fail, "1").unwrap();
    std::env::set_var("PISTACK_SHELF_DIR", &shelf);
    std::env::set_var("PISTACK_EXPORTS_FILE", dir.path().join("pistack.exports"));
    std::env::set_var("PISTACK_SHELF_EUID", "0");
    std::env::remove_var("PISTACK_SHELF_HELPER");
    std::env::set_var("PISTACK_EXPORTFS_LOG", &log);
    std::env::set_var("PISTACK_EXPORT_FAIL", &fail);
    std::env::set_var("PISTACK_SHELF_BACKOFF_MS", "800");
    std::env::set_var("PISTACK_SHELF_RETRY_MS", "100");
    std::env::set_var("PISTACK_CEPH_BIN", dir.path().join("no-ceph"));
    executable(
        dir.path(),
        "exportfs",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_EXPORTFS_LOG\"\n\
         if [ -f \"$PISTACK_EXPORT_FAIL\" ]; then echo boom >&2; exit 1; fi\nexit 0\n",
    );
    std::env::set_var("PISTACK_EXPORTFS", dir.path().join("exportfs"));

    let store = open_store(dir.path());
    let shutdown = pistack_types::module::CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    Shelf.start(ctx).await.expect("start shelf");
    let app = router(store);

    let (st, share) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "retry",
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "cidr": "10.3.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{share}");
    assert_eq!(share["status"]["phase"], "error");
    assert_ne!(share["status"]["phase"], "pending");
    let message = share["status"]["message"].as_str().unwrap();
    assert!(message.contains("boom"), "{message}");
    assert!(share["status"]["retry_at"].is_string(), "{share}");
    let id = share["id"].as_str().unwrap().to_owned();

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let calls = std::fs::read_to_string(&log).unwrap().lines().count();
    assert_eq!(calls, 1, "backoff holds the retry");

    std::fs::remove_file(&fail).unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(4);
    let mut ready = None;
    while tokio::time::Instant::now() < deadline {
        let (st, body) = call(
            &app,
            "GET",
            &format!("/shares/{id}"),
            None,
            Some(admin()),
            None,
        )
        .await;
        assert_eq!(st, StatusCode::OK, "{body}");
        if body["status"]["phase"] == "ready" {
            ready = Some(body);
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(40)).await;
    }
    let ready = ready.expect("reconcile loop retries the failed share");
    assert!(
        ready["status"]["message"].as_str().unwrap().contains(&id),
        "{ready}"
    );
    let calls = std::fs::read_to_string(&log).unwrap().lines().count();
    assert!(
        calls >= 2,
        "exportfs ran again after backoff, log lines {calls}"
    );
    stop_loop(shutdown).await;
    clear_share_tools();
}

#[tokio::test]
async fn non_root_exports_go_through_the_helper() {
    let _guard = shelf_lock().lock().await;
    let _path = RestoreEnv::set("PATH", std::env::var_os("PATH").unwrap_or_default());
    let dir = tempfile::tempdir().expect("tempdir");
    let shelf = dir.path().join("shares");
    let exports_path = dir.path().join("pistack.exports");
    let sudo_log = dir.path().join("sudo.log");
    let helper_log = dir.path().join("helper.log");
    let direct = dir.path().join("direct-exportfs");
    std::env::set_var("PISTACK_SHELF_DIR", &shelf);
    std::env::set_var("PISTACK_EXPORTS_FILE", &exports_path);
    std::env::set_var("PISTACK_SHELF_EUID", "1000");
    std::env::remove_var("PISTACK_SHELF_HELPER");
    std::env::set_var("PISTACK_CEPH_BIN", dir.path().join("no-ceph"));
    executable(
        dir.path(),
        "exportfs",
        "#!/bin/sh\ntouch \"$PISTACK_DIRECT_EXPORTFS\"\nexit 1\n",
    );
    std::env::set_var("PISTACK_EXPORTFS", dir.path().join("exportfs"));
    std::env::set_var("PISTACK_DIRECT_EXPORTFS", &direct);
    let app = router(open_store(dir.path()));

    let (st, blocked) = call(
        &app,
        "POST",
        "/shares",
        Some(json!({
            "name": "via-helper",
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "cidr": "10.5.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{blocked}");
    assert_eq!(blocked["status"]["phase"], "error");
    let message = blocked["status"]["message"].as_str().unwrap();
    assert!(message.contains("PISTACK_SHELF_HELPER"), "{message}");
    assert!(!direct.exists(), "direct exportfs ran without the helper");

    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let helper = executable(
        dir.path(),
        "helper",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_HELPER_LOG\"\n\
         file=\nprev=\nfor arg in \"$@\"; do\n\
         if [ \"$prev\" = \"--file\" ]; then file=$arg; fi\n\
         prev=$arg\ndone\n\
         mkdir -p \"$(dirname \"$file\")\"\ncat > \"$file\"\nexit 0\n",
    );
    executable(
        &bin,
        "sudo",
        "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_SUDO_LOG\"\n\
         if [ \"$1\" != \"-n\" ]; then echo expected -n >&2; exit 1; fi\n\
         shift\nprog=$1\nshift\nexec \"$prog\" \"$@\"\n",
    );
    let path = std::env::var_os("PATH").unwrap_or_default();
    let mut prefixed = std::ffi::OsString::from(bin);
    prefixed.push(":");
    prefixed.push(path);
    std::env::set_var("PATH", prefixed);
    std::env::set_var("PISTACK_SHELF_HELPER", &helper);
    std::env::set_var("PISTACK_SUDO_LOG", &sudo_log);
    std::env::set_var("PISTACK_HELPER_LOG", &helper_log);

    let id = blocked["id"].as_str().unwrap();
    let (st, share) = call(
        &app,
        "PUT",
        &format!("/shares/{id}"),
        Some(json!({
            "spec": {
                "protocol": "nfs",
                "size_gib": 1,
                "access": [{ "cidr": "10.5.0.0/24", "mode": "read" }]
            }
        })),
        Some(admin()),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{share}");
    assert_eq!(share["status"]["phase"], "ready", "{share}");
    assert!(!direct.exists(), "non-root path called exportfs directly");
    let sudo_text = std::fs::read_to_string(&sudo_log).unwrap();
    assert!(sudo_text.starts_with("-n\n"), "{sudo_text}");
    assert!(
        sudo_text.contains(&helper.display().to_string()),
        "{sudo_text}"
    );
    assert!(sudo_text.contains("apply"), "{sudo_text}");
    assert!(sudo_text.contains("--file"), "{sudo_text}");
    let helper_text = std::fs::read_to_string(&helper_log).unwrap();
    assert!(helper_text.contains("apply"), "{helper_text}");
    assert!(
        helper_text.contains(&exports_path.display().to_string()),
        "{helper_text}"
    );
    let share_dir = share["status"]["message"].as_str().unwrap();
    let line = format!("{share_dir} 10.5.0.0/24(ro,sync,no_subtree_check)");
    let lines = export_lines(&exports_path);
    assert_eq!(lines, vec![line], "{lines:?}");
    std::env::remove_var("PISTACK_DIRECT_EXPORTFS");
    clear_share_tools();
}
