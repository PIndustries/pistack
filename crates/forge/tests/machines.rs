// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end through the router: create a machine, watch the reconciler
//! place and start it on the local mock host, stop/start/reboot it, delete it.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use forge::{Forge, Host, Machine, Size};
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
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

async fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
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
    let app = forge.router(ctx.clone());
    forge.start(ctx).await.unwrap();
    // Seed and local host run under leadership asynchronously.
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let (st, sizes) = call::<Page<Size>>(&app, "GET", "/sizes", None).await;
        assert_eq!(st, StatusCode::OK);
        if sizes.total >= 4 {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "leader never seeded sizes"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    Harness { app, shutdown }
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

async fn raw(
    app: &Router,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, Vec<u8>) {
    let mut req = Request::builder().method(method).uri(path);
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

#[tokio::test]
async fn machine_lifecycle_on_local_mock_host() {
    let h = harness().await;
    let app = &h.app;

    // Seeded sizes and the dev host exist.
    let (st, sizes) = call::<Page<Size>>(app, "GET", "/sizes", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sizes.total, 4);
    let (st, hosts) = call::<Page<Host>>(app, "GET", "/hosts", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(hosts.items.len(), 1);
    assert_eq!(hosts.items[0].meta.name, "local");
    assert_eq!(hosts.items[0].status.phase, Phase::Ready);

    // Create.
    let body = serde_json::json!({
        "name": "web",
        "labels": {"tier": "web"},
        "spec": {"kind": "vm", "size": "m.2x4", "image": "ubuntu-24.04"}
    });
    let (st, m) = call::<Machine>(app, "POST", "/machines", Some(body)).await;
    assert_eq!(st, StatusCode::CREATED, "{m:?}");
    assert_eq!(m.status.phase, Phase::Pending);
    let id = m.meta.id.to_string();

    // Reconciler places and starts it.
    let m = wait_for(app, &id, |m| m.status.phase == Phase::Ready).await;
    assert_eq!(m.status.addresses, vec!["10.0.0.1"]);
    assert_eq!(m.status.host.as_ref().unwrap(), &hosts.items[0].meta.id);
    assert_eq!(m.status.power, forge::PowerState::On);

    // Host free capacity went down by the size.
    let (_, hosts2) = call::<Page<Host>>(app, "GET", "/hosts", None).await;
    let free = hosts2.items[0].status.free;
    assert_eq!(
        hosts.items[0].spec.capacity.memory_mib - free.memory_mib,
        4096
    );

    // Name resolution and list filters.
    let (st, by_name) = call::<Machine>(app, "GET", "/machines/web", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(by_name.meta.id.to_string(), id);
    let (_, page) = call::<Page<Machine>>(app, "GET", "/machines?labels=tier%3Dweb", None).await;
    assert_eq!(page.items.len(), 1);
    let (_, page) = call::<Page<Machine>>(app, "GET", "/machines?labels=tier%3Ddb", None).await;
    assert_eq!(page.items.len(), 0);

    // Stop, start, reboot.
    let (st, _) = call::<Machine>(app, "POST", &format!("/machines/{id}/stop"), None).await;
    assert_eq!(st, StatusCode::OK);
    wait_for(app, &id, |m| m.status.power == forge::PowerState::Off).await;
    let (st, _) = call::<Machine>(app, "POST", &format!("/machines/{id}/start"), None).await;
    assert_eq!(st, StatusCode::OK);
    wait_for(app, &id, |m| m.status.power == forge::PowerState::On).await;
    let (st, _) = call::<Machine>(app, "POST", &format!("/machines/{id}/reboot"), None).await;
    assert_eq!(st, StatusCode::OK);
    let m = wait_for(app, &id, |m| m.status.restarts == 1).await;
    assert_eq!(m.status.power, forge::PowerState::On);
    assert_eq!(m.status.addresses, vec!["10.0.0.1"]);

    // Stale If-Match on update conflicts.
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/machines/{id}"),
        Some(serde_json::json!({"description": "x"})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let (st, bytes) = {
        let req = Request::builder()
            .method("PUT")
            .uri(format!("/machines/{id}"))
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::IF_MATCH, "1")
            .body(Body::from(
                serde_json::to_vec(&serde_json::json!({"description": "y"})).unwrap(),
            ))
            .unwrap();
        let resp = app.clone().oneshot(req).await.unwrap();
        (
            resp.status(),
            resp.into_body().collect().await.unwrap().to_bytes(),
        )
    };
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );

    // Changing image after creation is rejected.
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/machines/{id}"),
        Some(serde_json::json!({"spec": {"kind": "vm", "size": "m.2x4", "image": "other"}})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);

    // Console returns a short-lived websocket ticket. The ticket is not logged.
    let (st, _) = raw(app, "GET", &format!("/machines/{id}/console"), None).await;
    assert_eq!(st, StatusCode::OK);

    // Delete: accepted, then gone.
    let (st, _) = raw(app, "DELETE", &format!("/machines/{id}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let (st, _) = raw(app, "GET", &format!("/machines/{id}"), None).await;
        if st == StatusCode::NOT_FOUND {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "machine never deleted"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    let (_, hosts3) = call::<Page<Host>>(app, "GET", "/hosts", None).await;
    assert_eq!(hosts3.items[0].status.free, hosts.items[0].spec.capacity);
}

#[tokio::test]
async fn validation_and_sizes() {
    let h = harness().await;
    let app = &h.app;

    let (st, _) = raw(
        app,
        "POST",
        "/machines",
        Some(serde_json::json!({"name": "Bad", "spec": {"size": "m.2x4", "image": "i"}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = raw(
        app,
        "POST",
        "/machines",
        Some(serde_json::json!({"name": "ok", "spec": {"size": "nope", "image": "i"}})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = raw(
        app,
        "POST",
        "/machines",
        Some(serde_json::json!({"name": "ok", "spec": {"size": "m.2x4", "image": ""}})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = raw(
        app,
        "POST",
        "/machines",
        Some(serde_json::json!({"not": "json shape"})),
    )
    .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // Volumes named at create are refused, for every kind, and nothing is
    // stored. An empty list is the same as leaving the field out.
    for kind in ["vm", "container", "metal"] {
        let (st, bytes) = raw(
            app,
            "POST",
            "/machines",
            Some(serde_json::json!({
                "name": "with-disk",
                "spec": {"kind": kind, "size": "m.2x4", "image": "i", "volumes": ["data"]},
            })),
        )
        .await;
        assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{kind}");
        let text = String::from_utf8_lossy(&bytes);
        assert!(text.contains("spec.volumes"), "{text}");
        assert!(text.contains("pi volume attach"), "{text}");
    }
    let (st, list) = call::<Page<Machine>>(app, "GET", "/machines", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(list.total, 0);
    let (st, m) = call::<Machine>(
        app,
        "POST",
        "/machines",
        Some(serde_json::json!({
            "name": "no-disk",
            "spec": {"size": "m.2x4", "image": "i", "volumes": []},
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    // Nor can an update add them.
    let mut spec = serde_json::to_value(&m.spec).unwrap();
    spec["volumes"] = serde_json::json!(["data"]);
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/machines/{}", m.meta.id),
        Some(serde_json::json!({ "spec": spec })),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = raw(app, "DELETE", &format!("/machines/{}", m.meta.id), None).await;
    assert!(st.is_success(), "{st}");

    // Size CRUD with a stale revision conflict.
    let (st, sz) = call::<Size>(
        app,
        "POST",
        "/sizes",
        Some(serde_json::json!({"name": "t.1x1", "vcpus": 1, "memory_mib": 1024})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = sz.meta.id.to_string();
    let (st, _) = raw(
        app,
        "POST",
        "/sizes",
        Some(serde_json::json!({"name": "t.1x1", "vcpus": 1, "memory_mib": 1024})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, sz2) = call::<Size>(
        app,
        "PUT",
        &format!("/sizes/{sid}"),
        Some(serde_json::json!({"name": "t.1x2", "vcpus": 1, "memory_mib": 2048})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(sz2.meta.revision, 2);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/sizes/{sid}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::IF_MATCH, "1")
        .body(Body::from(
            serde_json::to_vec(
                &serde_json::json!({"name": "t.1x2", "vcpus": 2, "memory_mib": 2048}),
            )
            .unwrap(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let (st, _) = call::<Page<Size>>(app, "GET", "/sizes?name=t.1x2", None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = raw(app, "DELETE", &format!("/sizes/{sid}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = raw(app, "GET", &format!("/sizes/{sid}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn unschedulable_machine_stays_pending_with_reason() {
    let h = harness().await;
    let app = &h.app;
    let body = serde_json::json!({
        "name": "huge",
        "spec": {"size": "xl.8x16", "image": "i", "host_selector": {"rack": "nowhere"}}
    });
    let (st, m) = call::<Machine>(app, "POST", "/machines", Some(body)).await;
    assert_eq!(st, StatusCode::CREATED);
    let id = m.meta.id.to_string();
    let m = wait_for(app, &id, |m| {
        m.status
            .conditions
            .iter()
            .any(|c| c.kind == "Scheduled" && !c.status)
    })
    .await;
    assert_eq!(m.status.phase, Phase::Pending);
    assert!(
        m.status.message.contains("selector"),
        "{}",
        m.status.message
    );
    // Deleting an unplaced machine works too.
    let (st, _) = raw(app, "DELETE", &format!("/machines/{id}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    while raw(app, "GET", &format!("/machines/{id}"), None).await.0 != StatusCode::NOT_FOUND {
        assert!(tokio::time::Instant::now() < deadline);
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
