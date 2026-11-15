// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `POST /machines/{id}/rebuild`: the refusals at the door, then a machine
//! that keeps its host, addresses and keys while its root disk is replaced
//! and first-boot setup is written again.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use forge::{Forge, Host, Machine, MockDriver, PowerState, REBUILD_ANNOTATION};
use http_body_util::BodyExt;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, Page, Phase};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Harness {
    app: Router,
    store: Store,
    /// Boot images, on the same store the API reads.
    mold: mold::Client,
    /// The dev host's driver, to make a call fail on purpose.
    drivers: Arc<forge::DriverRegistry>,
    shutdown: CancellationToken,
    _blobs: tempfile::TempDir,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn harness() -> Harness {
    let dir = tempfile::tempdir().unwrap();
    let mold = mold::Client::local(dir.path()).unwrap();
    let store = mold.store().clone();
    let shutdown = CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "testbox".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let forge = Forge::default();
    let drivers = forge.drivers();
    let app = forge.router(ctx.clone());
    forge.start(ctx).await.unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if store.collection::<forge::Size>().list(None).unwrap().len() >= 4 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "leader never seeded sizes"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Harness {
        app,
        store,
        mold,
        drivers,
        shutdown,
        _blobs: dir,
    }
}

async fn raw(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(path);
    if body.is_some() {
        // An action may be posted with no body at all; an empty JSON object
        // goes with the content type, as the CLI sends it.
    }
    let body = match body {
        Some(v) => {
            req = req.header(header::CONTENT_TYPE, "application/json");
            Body::from(serde_json::to_vec(&v).unwrap())
        }
        None => Body::empty(),
    };
    let resp = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = resp.status();
    let bytes = resp
        .into_body()
        .collect()
        .await
        .unwrap()
        .to_bytes()
        .to_vec();
    (status, bytes)
}

async fn call<T: DeserializeOwned>(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, T) {
    let (status, bytes) = raw(app, method, path, body).await;
    let v: T = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "{method} {path}: {status} bad body {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, v)
}

async fn wait_for(app: &Router, id: &str, pred: impl Fn(&Machine) -> bool) -> Machine {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (status, m) = call::<Machine>(app, "GET", &format!("/machines/{id}"), None).await;
        assert_eq!(status, StatusCode::OK);
        if pred(&m) {
            return m;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting; last: {m:?}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

/// A ready machine on the dev host, with ssh keys and user data to keep.
/// A catalog image ready for any project to boot, stored on `mold`'s own
/// store. The control plane and `mold` share it.
async fn put_image(mold: &mold::Client, name: &str) {
    let img = mold
        .create(mold::CreateImage {
            name: name.into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project: None,
            spec: mold::ImageSpec {
                format: mold::ImageFormat::Raw,
                os: None,
                arch: "x86_64".into(),
                min_disk_gib: 0,
                min_ram_mib: 0,
                visibility: mold::Visibility::Public,
                source_url: None,
            },
        })
        .unwrap();
    mold.import_reader(&img.meta.id, std::io::Cursor::new(vec![0u8; 1024]))
        .await
        .unwrap();
}

async fn ready_machine(app: &Router, name: &str) -> Machine {
    let body = serde_json::json!({
        "name": name,
        "spec": {
            "size": "m.2x4",
            "image": "ubuntu",
            "ssh_keys": ["ssh-ed25519 AAAA... bob@laptop"],
            "user_data": "#!/bin/sh\necho hello\n",
        }
    });
    let (st, m) = call::<Machine>(app, "POST", "/machines", Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{m:?}");
    let id = m.meta.id.to_string();
    wait_for(app, &id, |m| m.status.phase == Phase::Ready).await
}

#[tokio::test]
async fn a_rebuilt_machine_keeps_everything_but_its_root_disk() {
    let h = harness().await;
    let app = &h.app;
    put_image(&h.mold, "ubuntu").await;
    let m = ready_machine(app, "web").await;
    let id = m.meta.id.to_string();
    let host = m.status.host.clone();
    let address = m.status.addresses.clone();

    let (st, bytes) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));

    // Updating on the way through, ready again on the other side.
    let m = wait_for(app, &id, |m| {
        m.status.phase == Phase::Ready && !m.meta.annotations.contains_key(REBUILD_ANNOTATION)
    })
    .await;
    assert_eq!(m.status.host, host, "the machine stays on its host");
    assert_eq!(m.status.addresses, address, "ports and addresses are kept");
    assert_eq!(m.status.power, PowerState::On);
    assert_eq!(m.spec.image, "ubuntu");
    assert_eq!(m.spec.ssh_keys.len(), 1);
    assert!(m.spec.user_data.is_some());
    assert!(
        !m.meta.annotations.contains_key(REBUILD_ANNOTATION),
        "the request is cleared when it is done: {:?}",
        m.meta.annotations
    );
    // The dev host rebuilt, not kept: a new machine started there.
    let (st, hosts) = call::<Page<Host>>(app, "GET", "/hosts", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hosts.items.len(), 1);
}

#[tokio::test]
async fn a_stopped_machine_comes_back_stopped() {
    let h = harness().await;
    let app = &h.app;
    put_image(&h.mold, "ubuntu").await;
    let m = ready_machine(app, "idle").await;
    let id = m.meta.id.to_string();
    let (st, _) = raw(app, "POST", &format!("/machines/{id}/stop"), None).await;
    assert_eq!(st, StatusCode::OK);
    wait_for(app, &id, |m| m.status.power == PowerState::Off).await;

    let (st, m) = call::<Machine>(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{m:?}");
    let m = wait_for(app, &id, |m| {
        m.status.phase == Phase::Ready && m.status.power == PowerState::Off
    })
    .await;
    assert_eq!(m.spec.power, forge::DesiredPower::Off);
}

#[tokio::test]
async fn a_new_image_is_checked_before_the_request_is_taken() {
    let h = harness().await;
    let app = &h.app;
    put_image(&h.mold, "ubuntu").await;
    let m = ready_machine(app, "web").await;
    let id = m.meta.id.to_string();

    // Nothing of that name exists.
    let (st, bytes) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({"image": "nope"})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let text = String::from_utf8_lossy(&bytes);
    assert!(text.contains("not found"), "{text}");
    let (_, now) = call::<Machine>(app, "GET", &format!("/machines/{id}"), None).await;
    assert_eq!(now.spec.image, "ubuntu");
    assert_eq!(now.status.phase, Phase::Ready);

    // An empty name is a bad request.
    let (st, _) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({"image": "  "})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // The machine can be rebuilt again with its own image.
    let (st, _) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    wait_for(app, &id, |m| m.status.phase == Phase::Ready).await;
}

#[tokio::test]
async fn an_image_that_is_not_ready_or_not_visible_is_refused() {
    let h = harness().await;
    let app = &h.app;
    let mold = &h.mold;
    let meta_only = |name: &str, project: Option<pistack_types::Id>, vis: mold::Visibility| {
        mold.create(mold::CreateImage {
            name: name.into(),
            description: String::new(),
            labels: Default::default(),
            annotations: Default::default(),
            project,
            spec: mold::ImageSpec {
                format: mold::ImageFormat::Raw,
                os: None,
                arch: "x86_64".into(),
                min_disk_gib: 0,
                min_ram_mib: 0,
                visibility: vis,
                source_url: None,
            },
        })
        .unwrap()
    };
    put_image(mold, "noble").await;
    // Ready, but another project's and private.
    let other = pistack_types::Id::new(pistack_types::kind::PROJECT);
    let secret = meta_only("secret", Some(other), mold::Visibility::Private);
    mold.images()
        .modify(&secret.meta.id, |i| {
            i.status.phase = Phase::Ready;
            Ok(())
        })
        .unwrap();
    // Named and visible, but never uploaded.
    meta_only("pending", None, mold::Visibility::Public);
    // Ready and visible, but wants a bigger disk than the size gives.
    put_image(mold, "huge").await;
    let big = mold.images().get_by_name(None, "huge").unwrap();
    mold.images()
        .modify(&big.meta.id, |i| {
            i.spec.min_disk_gib = 40;
            Ok(())
        })
        .unwrap();

    let m = ready_machine(app, "web").await;
    let id = m.meta.id.to_string();

    let (st, bytes) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({"image": "noble"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let m = wait_for(app, &id, |m| m.status.phase == Phase::Ready).await;
    assert_eq!(m.spec.image, "noble");

    for (name, want) in [
        ("secret", "not found"),
        ("pending", "not ready"),
        ("huge", "disk"),
        ("nowhere", "not found"),
    ] {
        let (st, bytes) = raw(
            app,
            "POST",
            &format!("/machines/{id}/rebuild"),
            Some(serde_json::json!({"image": name})),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{name}");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains(want), "{name}: {text}");
        let (_, now) = call::<Machine>(app, "GET", &format!("/machines/{id}"), None).await;
        assert_eq!(now.spec.image, "noble", "{name} must not be stored");
        assert_eq!(now.status.phase, Phase::Ready, "{name} must not start");
    }
}

#[tokio::test]
async fn a_machine_being_deleted_or_mid_flight_is_refused() {
    let h = harness().await;
    let app = &h.app;
    put_image(&h.mold, "ubuntu").await;
    let m = ready_machine(app, "web").await;
    let id = m.meta.id.to_string();

    // A rebuild in flight cannot be asked for twice.
    let (st, _) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, bytes) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert!(
        st == StatusCode::CONFLICT || st == StatusCode::OK,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    // Either way the machine is rebuilt once and comes back whole.
    let m = wait_for(app, &id, |m| m.status.phase == Phase::Ready).await;
    assert!(!m.meta.annotations.contains_key(REBUILD_ANNOTATION));

    // A delete ends the machine; a rebuild after it is refused.
    let (st, _) = raw(app, "DELETE", &format!("/machines/{id}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    let (st, _) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
}

#[tokio::test]
async fn a_rebuild_of_a_pending_machine_happens_after_placement() {
    let h = harness().await;
    let app = &h.app;
    put_image(&h.mold, "ubuntu").await;
    // Too large for the dev host is not schedulable; ask for a zone that
    // no host has instead, so the machine stays pending with no host.
    let body = serde_json::json!({
        "name": "later",
        "spec": {"size": "s.1x1", "image": "ubuntu", "host_selector": {"zone": "nowhere"}}
    });
    let (st, m) = call::<Machine>(app, "POST", "/machines", Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{m:?}");
    let id = m.meta.id.to_string();

    let (st, m) = call::<Machine>(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{m:?}");
    assert_eq!(m.status.phase, Phase::Pending);
    assert_eq!(
        m.meta
            .annotations
            .get(REBUILD_ANNOTATION)
            .map(String::as_str),
        Some("ubuntu")
    );

    // Free of its selector, it is placed and then rebuilt, not started twice.
    h.store
        .collection::<Machine>()
        .modify(&m.meta.id, |x| {
            x.spec.host_selector.clear();
            Ok(())
        })
        .unwrap();
    let m = wait_for(app, &id, |m| m.status.phase == Phase::Ready).await;
    assert_eq!(m.status.power, PowerState::On);
    assert!(!m.meta.annotations.contains_key(REBUILD_ANNOTATION));
}

#[tokio::test]
async fn a_failed_rebuild_leaves_the_machine_startable_and_rebuildable() {
    let h = harness().await;
    let app = &h.app;
    put_image(&h.mold, "ubuntu").await;
    let m = ready_machine(app, "web").await;
    let id = m.meta.id.to_string();
    let host = m.status.host.clone().unwrap();

    // The dev host's driver fails the next rebuild, as a hypervisor whose
    // disk cannot be written does.
    let drv = h.drivers.get(&host).expect("the dev host has a driver");
    let driver = (drv.as_ref() as &dyn std::any::Any)
        .downcast_ref::<MockDriver>()
        .expect("the dev host has a mock driver");
    driver.fail_next_rebuild();

    let (st, bytes) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let m = wait_for(app, &id, |m| {
        !m.meta.annotations.contains_key(REBUILD_ANNOTATION)
            && m.status
                .conditions
                .iter()
                .any(|c| c.kind == "Running" && c.reason == "RebuildFailed")
    })
    .await;
    // The old disk is still there, so the machine is not filed away: it is
    // ready, the failure is a message, and it may be rebuilt again.
    assert_eq!(m.status.phase, Phase::Ready, "{}", m.status.message);
    assert!(m.status.message.contains("disk"), "{}", m.status.message);
    assert_eq!(m.status.power, PowerState::On);

    let (st, bytes) = raw(
        app,
        "POST",
        &format!("/machines/{id}/rebuild"),
        Some(serde_json::json!({})),
    )
    .await;
    assert_eq!(st, StatusCode::OK, "{}", String::from_utf8_lossy(&bytes));
    let m = wait_for(app, &id, |m| {
        !m.meta.annotations.contains_key(REBUILD_ANNOTATION)
            && m.status
                .conditions
                .iter()
                .any(|c| c.kind == "Running" && c.reason == "Rebuilt")
    })
    .await;
    assert_eq!(m.status.phase, Phase::Ready);
}
