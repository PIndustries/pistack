// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host maintenance through the router: an admin labels, disables and
//! enables hosts, and the scheduler follows.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use forge::{new_host, DriverRegistry, Forge, Host, Machine, MockDriver};
use http_body_util::BodyExt;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext, Phase, Resources};
use serde::de::DeserializeOwned;
use serde_json::json;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Harness {
    app: Router,
    store: Store,
    drivers: Arc<DriverRegistry>,
    shutdown: CancellationToken,
}

impl Drop for Harness {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// A control plane with no hosts. Requests need a caller.
async fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let shutdown = CancellationToken::new();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: shutdown.clone(),
        public_url: "http://test".into(),
        node_name: "control".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let drivers = Arc::new(DriverRegistry::new());
    let forge = Forge::with_registry(drivers.clone());
    let app = forge.router(ctx.clone());
    Forge::seed_sizes(&store).unwrap();
    forge.start(ctx).await.unwrap();
    Harness {
        app,
        store,
        drivers,
        shutdown,
    }
}

impl Harness {
    /// A ready host served by a mock driver in this process.
    fn add_host(&self, name: &str) -> (Host, Arc<MockDriver>) {
        let mut labels = BTreeMap::new();
        labels.insert("pistack.io/agent".to_owned(), "true".to_owned());
        let host = self
            .store
            .collection::<Host>()
            .create(new_host(
                name,
                name,
                Resources {
                    vcpus: 8,
                    memory_mib: 16384,
                    disk_gib: 200,
                    gpus: 0,
                },
                vec!["mock".into()],
                labels,
            ))
            .unwrap();
        let driver = Arc::new(MockDriver::new());
        self.drivers
            .register_local(host.meta.id.clone(), driver.clone());
        (host, driver)
    }

    async fn machine(&self, name: &str, spec: serde_json::Value) -> Machine {
        let (st, m) = call::<Machine>(
            &self.app,
            &admin(),
            "POST",
            "/machines",
            Some(json!({"name": name, "spec": spec})),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        m
    }

    async fn wait(&self, id: &Id, pred: impl Fn(&Machine) -> bool) -> Machine {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        loop {
            let m = self.store.collection::<Machine>().get(id).unwrap();
            if pred(&m) {
                return m;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out waiting; last: {m:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn ready_on(&self, id: &Id) -> Id {
        self.wait(id, |m| m.status.phase == Phase::Ready)
            .await
            .status
            .host
            .clone()
            .expect("a ready machine has a host")
    }
}

fn admin() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "root".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

/// `admin` inside one project: not a cloud admin.
fn project_admin() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "owner".into(),
        project: Some(Id::new(kind::PROJECT)),
        roles: vec!["admin".into()],
        system: false,
    }
}

async fn raw(
    app: &Router,
    caller: &Caller,
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
    let mut req = req.body(body).unwrap();
    req.extensions_mut().insert(caller.clone());
    let resp = app.clone().oneshot(req).await.unwrap();
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
    caller: &Caller,
    method: &str,
    path: &str,
    body: Option<serde_json::Value>,
) -> (StatusCode, T) {
    let (status, bytes) = raw(app, caller, method, path, body).await;
    let v: T = serde_json::from_slice(&bytes).unwrap_or_else(|e| {
        panic!(
            "{method} {path}: {status} bad body {e}: {}",
            String::from_utf8_lossy(&bytes)
        )
    });
    (status, v)
}

fn vm(extra: serde_json::Value) -> serde_json::Value {
    let mut spec = json!({"size": "s.1x1", "image": "ubuntu"});
    for (k, v) in extra.as_object().unwrap() {
        spec[k] = v.clone();
    }
    spec
}

#[tokio::test]
async fn labels_set_through_the_api_steer_the_scheduler() {
    let h = harness().await;
    let (a, _) = h.add_host("a");
    let (b, _) = h.add_host("b");

    // Only a cloud admin may write labels. Admin of a project is not one.
    let labels = json!({"labels": {"zone": "west", "rack": "r2"}});
    let (st, _) = raw(
        &h.app,
        &project_admin(),
        "PUT",
        "/hosts/b/labels",
        Some(labels.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let stored = h.store.collection::<Host>().get(&b.meta.id).unwrap();
    assert!(!stored.meta.labels.contains_key("zone"));

    let (st, host) = call::<Host>(&h.app, &admin(), "PUT", "/hosts/b/labels", Some(labels)).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(host.meta.labels.get("zone").unwrap(), "west");
    assert_eq!(host.meta.labels.get("rack").unwrap(), "r2");
    // The label PiStack wrote itself is still there.
    assert_eq!(host.meta.labels.get("pistack.io/agent").unwrap(), "true");

    // A label under pistack.io/ cannot be written, and a bad one is refused.
    for bad in [
        json!({"labels": {"pistack.io/agent": "false"}}),
        json!({"labels": {"zone": ""}}),
        json!({"labels": {"zo ne": "west"}}),
    ] {
        let (st, _) = raw(&h.app, &admin(), "PUT", "/hosts/b/labels", Some(bad)).await;
        assert!(st.is_client_error(), "{st}");
    }
    let stored = h.store.collection::<Host>().get(&b.meta.id).unwrap();
    assert_eq!(stored.meta.labels.get("zone").unwrap(), "west");

    // A selector on either label lands on the labelled host.
    for (name, selector) in [
        ("in-zone", json!({"zone": "west"})),
        ("in-rack", json!({"rack": "r2", "zone": "west"})),
    ] {
        let m = h
            .machine(name, vm(json!({ "host_selector": selector })))
            .await;
        assert_eq!(h.ready_on(&m.meta.id).await, b.meta.id);
    }

    // A label no host has leaves the machine pending and says why.
    let lost = h
        .machine("nowhere", vm(json!({"host_selector": {"zone": "north"}})))
        .await;
    let lost = h
        .wait(&lost.meta.id, |m| {
            m.status
                .conditions
                .iter()
                .any(|c| c.kind == "Scheduled" && !c.status)
        })
        .await;
    assert_eq!(lost.status.phase, Phase::Pending);
    assert!(
        lost.status.message.contains("selector"),
        "{}",
        lost.status.message
    );

    // Replacing the labels moves the zone: b loses it, a gains it.
    let (st, host) = call::<Host>(
        &h.app,
        &admin(),
        "PUT",
        "/hosts/b/labels",
        Some(json!({"labels": {}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert!(!host.meta.labels.contains_key("zone"));
    assert!(!host.meta.labels.contains_key("rack"));
    assert!(host.meta.labels.contains_key("pistack.io/agent"));
    let (st, _) = raw(
        &h.app,
        &admin(),
        "PUT",
        &format!("/hosts/{}/labels", a.meta.id),
        Some(json!({"labels": {"zone": "west"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let m = h
        .machine("moved", vm(json!({"host_selector": {"zone": "west"}})))
        .await;
    assert_eq!(h.ready_on(&m.meta.id).await, a.meta.id);
}

#[tokio::test]
async fn a_machine_zone_is_a_selector_on_the_zone_label() {
    let h = harness().await;
    let (_, _) = h.add_host("a");
    let (b, _) = h.add_host("b");
    let (st, _) = raw(
        &h.app,
        &admin(),
        "PUT",
        "/hosts/b/labels",
        Some(json!({"labels": {"zone": "west"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let m = h.machine("in-zone", vm(json!({"zone": "west"}))).await;
    assert_eq!(m.spec.zone.as_deref(), Some("west"));
    assert_eq!(h.ready_on(&m.meta.id).await, b.meta.id);

    // A zone no host has leaves the machine pending and says why.
    let lost = h.machine("nowhere", vm(json!({"zone": "north"}))).await;
    let lost = h
        .wait(&lost.meta.id, |m| {
            m.status
                .conditions
                .iter()
                .any(|c| c.kind == "Scheduled" && !c.status)
        })
        .await;
    assert_eq!(lost.status.phase, Phase::Pending);
    assert!(
        lost.status.message.contains("selector"),
        "{}",
        lost.status.message
    );

    // A zone that disagrees with the selector can match nothing.
    for spec in [
        vm(json!({"zone": "west", "host_selector": {"zone": "east"}})),
        vm(json!({"zone": " "})),
    ] {
        let (st, _) = raw(
            &h.app,
            &admin(),
            "POST",
            "/machines",
            Some(json!({"name": "torn", "spec": spec})),
        )
        .await;
        assert!(st.is_client_error(), "{st}");
    }
}

#[tokio::test]
async fn a_disabled_host_keeps_its_machines_and_takes_no_new_ones() {
    let h = harness().await;
    let (a, a_driver) = h.add_host("a");
    let (b, _) = h.add_host("b");

    // Equal hosts tie to the older one, so the first machine is on a.
    let first = h.machine("first", vm(json!({}))).await;
    assert_eq!(h.ready_on(&first.meta.id).await, a.meta.id);

    let reason = json!({"reason": "replacing a fan"});
    let (st, _) = raw(
        &h.app,
        &project_admin(),
        "POST",
        "/hosts/a/disable",
        Some(reason.clone()),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, host) = call::<Host>(&h.app, &admin(), "POST", "/hosts/a/disable", Some(reason)).await;
    assert_eq!(st, StatusCode::OK);
    assert!(host.spec.disabled);
    assert_eq!(host.spec.disabled_reason, "replacing a fan");
    // Disabling again with no body keeps the reason.
    let (st, host) = call::<Host>(&h.app, &admin(), "POST", "/hosts/a/disable", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(host.spec.disabled_reason, "replacing a fan");
    // The host itself is still healthy.
    assert_eq!(host.status.phase, Phase::Ready);

    // a is emptier, but everything new goes to b.
    for name in ["n1", "n2", "n3"] {
        let m = h.machine(name, vm(json!({}))).await;
        assert_eq!(h.ready_on(&m.meta.id).await, b.meta.id);
    }

    // The machine that was on a is still there and can still be driven.
    let id = first.meta.id.to_string();
    let (st, _) = raw(
        &h.app,
        &admin(),
        "POST",
        &format!("/machines/{id}/stop"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    h.wait(&first.meta.id, |m| m.status.power == forge::PowerState::Off)
        .await;
    let (st, _) = raw(
        &h.app,
        &admin(),
        "POST",
        &format!("/machines/{id}/start"),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let m = h
        .wait(&first.meta.id, |m| m.status.power == forge::PowerState::On)
        .await;
    assert_eq!(m.status.host.as_ref(), Some(&a.meta.id));
    assert_eq!(a_driver.len(), 1);

    // With every host disabled a machine waits and says so.
    let (st, _) = raw(&h.app, &admin(), "POST", "/hosts/b/disable", None).await;
    assert_eq!(st, StatusCode::OK);
    let waiting = h.machine("waiting", vm(json!({}))).await;
    let waiting = h
        .wait(&waiting.meta.id, |m| {
            m.status
                .conditions
                .iter()
                .any(|c| c.kind == "Scheduled" && !c.status)
        })
        .await;
    assert!(
        waiting.status.message.contains("2 disabled"),
        "{}",
        waiting.status.message
    );

    // Enabling a host clears the reason and the machine lands there.
    let (st, _) = raw(&h.app, &project_admin(), "POST", "/hosts/a/enable", None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    let (st, host) = call::<Host>(&h.app, &admin(), "POST", "/hosts/a/enable", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(!host.spec.disabled);
    assert!(host.spec.disabled_reason.is_empty());
    assert_eq!(h.ready_on(&waiting.meta.id).await, a.meta.id);
}
