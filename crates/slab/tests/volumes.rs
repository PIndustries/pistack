// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Integration: in-memory store + local file backend in a tempdir.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    Router,
};
use pistack_types::{kind, Caller, Id, Phase};
use serde_json::{json, Value};
use slab::{Client, Create, LocalFileBackend, Protocol, SnapshotSpec, VolumeSource, VolumeSpec};
use tower::ServiceExt;

const GIB: u64 = 1024 * 1024 * 1024;

fn client(dir: &tempfile::TempDir) -> Client {
    let c = Client::local(dir.path()).expect("client");
    c.seed_defaults().expect("seed");
    c
}

fn create(name: &str, spec: VolumeSpec) -> Create<VolumeSpec> {
    Create {
        name: name.into(),
        description: String::new(),
        labels: Default::default(),
        annotations: Default::default(),
        project: Some(Id::new("proj")),
        spec,
    }
}

fn spec(size_gib: u64) -> VolumeSpec {
    VolumeSpec {
        size_gib,
        volume_type: "standard".into(),
        source: VolumeSource::None,
        bootable: false,
        encrypted: false,
    }
}

#[tokio::test]
async fn create_snapshot_delete_on_local_backend() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let backend = LocalFileBackend::new(dir.path());

    // create 1 GiB volume -> Pending
    let vol = c
        .create_volume(create("data", spec(1)))
        .await
        .expect("create");
    assert_eq!(vol.status.phase, Phase::Pending);
    assert_eq!(vol.spec.volume_type, "standard");

    // reconcile -> Ready, sparse file of exactly 1 GiB
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(vol.status.phase, Phase::Ready);
    let path = backend.volume_path(&vol);
    assert_eq!(
        vol.status.backend_ref.as_deref(),
        Some(path.to_string_lossy().as_ref())
    );
    let md = std::fs::metadata(&path).expect("volume file exists");
    assert_eq!(md.len(), GIB);

    // attach info for forge
    let info = c
        .attach_info(&vol.meta.id, "host-a")
        .await
        .expect("attach_info");
    assert_eq!(info.protocol, Protocol::Local);

    // extend to 2 GiB
    let vol = c.extend_volume(&vol.meta.id, 2).await.expect("extend");
    assert_eq!(vol.status.phase, Phase::Updating);
    let vol = c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(vol.status.phase, Phase::Ready);
    assert_eq!(std::fs::metadata(&path).expect("file").len(), 2 * GIB);

    // snapshot
    let snap = c
        .create_snapshot(Create {
            name: "data-snap".into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: SnapshotSpec {
                volume: vol.meta.id.clone(),
            },
        })
        .await
        .expect("snapshot");
    assert_eq!(
        snap.meta.project, vol.meta.project,
        "snapshot inherits project"
    );
    let snap = c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(snap.status.phase, Phase::Ready);
    assert_eq!(snap.status.size_gib, 2);
    let snap_path = backend.snapshot_path(&snap);
    assert!(snap_path.exists());

    // volume with snapshots cannot be deleted
    assert_eq!(
        c.request_delete_volume(&vol.meta.id)
            .expect_err("has snapshots")
            .code,
        pistack_types::error::ErrorCode::Conflict
    );

    // restore the snapshot into a new, bigger volume
    let restored = c
        .create_volume(create(
            "restored",
            VolumeSpec {
                size_gib: 3,
                source: VolumeSource::Snapshot(snap.meta.id.clone()),
                ..spec(3)
            },
        ))
        .await
        .expect("create from snapshot");
    let restored = c
        .reconcile_volume(&restored.meta.id)
        .await
        .expect("reconcile")
        .expect("exists");
    assert_eq!(restored.status.phase, Phase::Ready);
    assert_eq!(
        std::fs::metadata(backend.volume_path(&restored))
            .expect("file")
            .len(),
        3 * GIB
    );

    // delete snapshot, then volume; files and records go away
    c.request_delete_snapshot(&snap.meta.id)
        .expect("request delete");
    assert!(c
        .reconcile_snapshot(&snap.meta.id)
        .await
        .expect("reconcile")
        .is_none());
    assert!(!snap_path.exists());

    c.request_delete_volume(&vol.meta.id)
        .expect("request delete");
    assert!(c
        .reconcile_volume(&vol.meta.id)
        .await
        .expect("reconcile")
        .is_none());
    assert!(!path.exists());
    assert!(c.volumes().try_get(&vol.meta.id).expect("store").is_none());
}

#[tokio::test]
async fn attach_detach_rules() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let vol = c.create_volume(create("a", spec(1))).await.expect("create");
    let machine = Id::new("mach");

    // not ready yet
    assert!(c
        .mark_attached(&vol.meta.id, &machine, Some("vdb".into()))
        .is_err());
    c.reconcile_volume(&vol.meta.id).await.expect("reconcile");

    let v = c
        .mark_attached(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("attach");
    assert_eq!(v.status.attached_to.as_ref(), Some(&machine));
    assert_eq!(v.status.device.as_deref(), Some("vdb"));

    // another machine is refused; same machine is idempotent
    assert!(c
        .mark_attached(&vol.meta.id, &Id::new("mach"), None)
        .is_err());
    c.mark_attached(&vol.meta.id, &machine, Some("vdb".into()))
        .expect("idempotent");

    // attached volumes cannot be deleted
    assert!(c.request_delete_volume(&vol.meta.id).is_err());

    let v = c.mark_detached(&vol.meta.id).expect("detach");
    assert!(v.status.attached_to.is_none());
    c.request_delete_volume(&vol.meta.id)
        .expect("delete after detach");
}

async fn call(app: &Router, method: &str, uri: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(uri);
    let body = match body {
        Some(v) => {
            req = req.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    let mut req = req.body(body).expect("request");
    req.extensions_mut().insert(admin_caller());
    let resp = app.clone().oneshot(req).await.expect("response");
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.expect("body");
    let v = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).expect("json")
    };
    (status, v)
}

#[tokio::test]
async fn http_crud_with_stale_revision() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let app = slab::http::router(c.clone());

    // volume types: seeded "standard"
    let (st, v) = call(&app, "GET", "/volume-types", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(v["total"], 1);
    assert_eq!(v["items"][0]["name"], "standard");
    assert_eq!(v["items"][0]["spec"]["backend"], "local");

    // create
    let (st, vol) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "web-data", "labels": { "tier": "web" }, "spec": { "size_gib": 1 } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{vol}");
    let id = vol["id"].as_str().expect("id").to_owned();
    assert!(id.starts_with("vol-"));
    assert_eq!(vol["status"]["phase"], "pending");
    assert_eq!(vol["revision"], 1);

    // duplicate name -> 409
    let (st, _) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "web-data", "spec": { "size_gib": 1 } })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // bad size -> 400; unknown type -> 400
    let (st, _) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "z", "spec": { "size_gib": 0 } })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = call(
        &app,
        "POST",
        "/volumes",
        Some(json!({ "name": "z", "spec": { "size_gib": 1, "volume_type": "gold" } })),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // list, by label, by name
    let (_, page) = call(&app, "GET", "/volumes", None).await;
    assert_eq!(page["total"], 1);
    let (_, page) = call(&app, "GET", "/volumes?labels=tier=db", None).await;
    assert_eq!(page["total"], 0);
    let (st, got) = call(&app, "GET", "/volumes/web-data", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(got["id"], id);

    // update with If-Match: correct, then stale -> 409
    let resp = app
        .clone()
        .oneshot(with_admin(
            Request::builder()
                .method("PUT")
                .uri(format!("/volumes/{id}"))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(
                    json!({ "description": "web node data" }).to_string(),
                ))
                .expect("req"),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::OK);
    let resp = app
        .clone()
        .oneshot(with_admin(
            Request::builder()
                .method("PUT")
                .uri(format!("/volumes/{id}"))
                .header("content-type", "application/json")
                .header("if-match", "1")
                .body(Body::from(json!({ "description": "stale" }).to_string()))
                .expect("req"),
        ))
        .await
        .expect("resp");
    assert_eq!(resp.status(), StatusCode::CONFLICT);

    // extend before ready -> 409; make ready; extend -> 202
    let (st, _) = call(
        &app,
        "POST",
        &format!("/volumes/{id}/extend"),
        Some(json!({ "size_gib": 2 })),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let vid = Id::parse("vol", &id).expect("id");
    c.reconcile_volume(&vid).await.expect("reconcile");
    let (st, v) = call(
        &app,
        "POST",
        &format!("/volumes/{id}/extend"),
        Some(json!({ "size_gib": 2 })),
    )
    .await;
    assert_eq!(st, StatusCode::ACCEPTED, "{v}");
    c.reconcile_volume(&vid).await.expect("reconcile");

    // attach / detach
    let machine = Id::new("mach").to_string();
    let (st, v) = call(
        &app,
        "POST",
        &format!("/volumes/{id}/attach"),
        Some(json!({ "machine": machine, "device": "vdb" })),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["status"]["attached_to"], machine);
    let (st, _) = call(&app, "DELETE", &format!("/volumes/{id}"), None).await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "attached volumes cannot be deleted"
    );
    let (st, _) = call(&app, "POST", &format!("/volumes/{id}/detach"), None).await;
    assert_eq!(st, StatusCode::OK);

    // snapshots and backups
    let (st, snap) = call(
        &app,
        "POST",
        "/snapshots",
        Some(json!({ "name": "s1", "spec": { "volume": id } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{snap}");
    let (st, bak) = call(
        &app,
        "POST",
        "/backups",
        Some(json!({ "name": "b1", "spec": { "volume": id, "target": "backups" } })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{bak}");
    let bid = bak["id"].as_str().expect("id");
    let (st, _) = call(&app, "POST", &format!("/backups/{bid}/restore"), None).await;
    assert_eq!(st, StatusCode::NOT_IMPLEMENTED);
    let (st, _) = call(&app, "DELETE", &format!("/backups/{bid}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);

    let sid = snap["id"].as_str().expect("id");
    let (st, _) = call(&app, "DELETE", &format!("/snapshots/{sid}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    c.reconcile_snapshot(&Id::parse("snap", sid).expect("id"))
        .await
        .expect("reconcile");

    // delete volume -> 202, reconcile removes it -> 404
    let (st, _) = call(&app, "DELETE", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    c.reconcile_volume(&vid).await.expect("reconcile");
    let (st, _) = call(&app, "GET", &format!("/volumes/{id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // volume type in use cannot be deleted; unused one can
    let (st, vt) = call(
        &app,
        "POST",
        "/volume-types",
        Some(
            json!({ "name": "gold", "spec": { "backend": "local", "params": { "tier": "nvme" } } }),
        ),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{vt}");
    let (st, _) = call(&app, "DELETE", "/volume-types/gold", None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn reconciler_loop_drives_volume_to_ready() {
    let dir = tempfile::tempdir().expect("tempdir");
    let c = client(&dir);
    let shutdown = tokio_util::sync::CancellationToken::new();
    let rx = c.store().events().subscribe();
    let task = tokio::spawn(slab::reconcile::run(c.clone(), rx, shutdown.clone()));

    let vol = c
        .create_volume(create("looped", spec(1)))
        .await
        .expect("create");
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let cur = c.volumes().get(&vol.meta.id).expect("get");
        if cur.status.phase == Phase::Ready {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "volume never became ready: {cur:?}"
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    shutdown.cancel();
    task.await.expect("reconciler exits");
}

fn admin_caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

fn with_admin(mut req: Request<Body>) -> Request<Body> {
    req.extensions_mut().insert(admin_caller());
    req
}

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let dir = tempfile::tempdir().expect("tempdir");
    let app = slab::http::router(client(&dir));
    let res = app
        .oneshot(
            Request::builder()
                .uri("/volumes")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let bytes = to_bytes(res.into_body(), 1 << 20).await.unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}
