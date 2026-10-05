// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! DaemonSet controller: one pod per eligible host, cleanup on leave.

use forge::{new_host, Host};
use loom::controllers::{daemonset::DaemonSetController, Controller};
use loom::{
    Container, DaemonSet, DaemonSetSpec, Pod, PodSpec, PodTemplate, StatefulSet, DELETE_ANNOTATION,
    HOSTNAME_LABEL, OWNER_LABEL, POD_TEMPLATE_HASH, TARGET_HOST_ANNOTATION,
};
use pistack_store::Store;
use pistack_types::{Phase, Resource, Resources};
use std::collections::BTreeMap;

fn template() -> PodTemplate {
    PodTemplate {
        labels: [("app".into(), "agent".into())].into(),
        annotations: Default::default(),
        spec: PodSpec {
            containers: vec![Container {
                name: "agent".into(),
                image: "agent:1".into(),
                ..Default::default()
            }],
            ..Default::default()
        },
    }
}

fn host(name: &str, drivers: &[&str], labels: BTreeMap<String, String>) -> Host {
    let mut h = new_host(
        name,
        name,
        Resources {
            vcpus: 8,
            memory_mib: 16_384,
            disk_gib: 100,
            gpus: 0,
        },
        drivers.iter().map(|s| s.to_string()).collect(),
        labels,
    );
    h.status.phase = Phase::Ready;
    h
}

#[tokio::test]
async fn one_pod_per_eligible_host() {
    let store = Store::in_memory().unwrap();
    let hosts = store.collection::<Host>();
    let sets = store.collection::<DaemonSet>();
    let pods = store.collection::<Pod>();
    let ctl = DaemonSetController::new(store.clone());

    let a = hosts
        .create(host("node-a", &["mock"], BTreeMap::new()))
        .unwrap();
    let b = hosts
        .create(host("node-b", &["youki"], BTreeMap::new()))
        .unwrap();
    // No container driver: skipped.
    hosts
        .create(host("node-vm", &["libvirt"], BTreeMap::new()))
        .unwrap();

    let ds = sets
        .create(Resource::new(
            "logger",
            None,
            DaemonSetSpec {
                selector: Default::default(),
                template: template(),
            },
        ))
        .unwrap();

    ctl.reconcile_once().await.unwrap();
    let list = pods.list(None).unwrap();
    assert_eq!(
        list.len(),
        2,
        "{:?}",
        list.iter().map(|p| &p.meta.name).collect::<Vec<_>>()
    );
    for p in &list {
        assert_eq!(
            p.meta.labels.get(OWNER_LABEL).unwrap(),
            &ds.meta.id.to_string()
        );
        assert!(p.meta.labels.contains_key(POD_TEMPLATE_HASH));
        let hid = p.meta.annotations.get(TARGET_HOST_ANNOTATION).unwrap();
        assert!(hid == &a.meta.id.to_string() || hid == &b.meta.id.to_string());
        assert_eq!(
            p.spec.node_selector.get(HOSTNAME_LABEL).unwrap(),
            if hid == &a.meta.id.to_string() {
                "node-a"
            } else {
                "node-b"
            }
        );
    }

    // Hostname labels stamped on hosts.
    let a2 = hosts.get(&a.meta.id).unwrap();
    assert_eq!(a2.meta.labels.get(HOSTNAME_LABEL).unwrap(), "node-a");

    let ds = sets.get(&ds.meta.id).unwrap();
    assert_eq!(ds.status.desired, 2);
    assert_eq!(ds.status.current, 2);

    // Host leaves: its pod is deleted.
    hosts
        .modify(&b.meta.id, |h| {
            h.status.phase = Phase::Error;
            Ok(())
        })
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    let live: Vec<_> = pods
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|p| !p.meta.annotations.contains_key(DELETE_ANNOTATION))
        .collect();
    assert_eq!(live.len(), 1);
    assert_eq!(
        live[0]
            .meta
            .annotations
            .get(TARGET_HOST_ANNOTATION)
            .unwrap(),
        &a.meta.id.to_string()
    );
}

#[tokio::test]
async fn respects_node_selector() {
    let store = Store::in_memory().unwrap();
    let hosts = store.collection::<Host>();
    let sets = store.collection::<DaemonSet>();
    let pods = store.collection::<Pod>();
    let ctl = DaemonSetController::new(store.clone());

    hosts
        .create(host(
            "east",
            &["mock"],
            [("zone".into(), "east".into())].into(),
        ))
        .unwrap();
    hosts
        .create(host(
            "west",
            &["mock"],
            [("zone".into(), "west".into())].into(),
        ))
        .unwrap();

    let mut t = template();
    t.spec.node_selector.insert("zone".into(), "east".into());
    sets.create(Resource::new(
        "east-only",
        None,
        DaemonSetSpec {
            selector: Default::default(),
            template: t,
        },
    ))
    .unwrap();

    ctl.reconcile_once().await.unwrap();
    let list = pods.list(None).unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(
        list[0].spec.node_selector.get(HOSTNAME_LABEL).unwrap(),
        "east"
    );
    assert_eq!(list[0].spec.node_selector.get("zone").unwrap(), "east");
}

#[tokio::test]
async fn host_join_creates_pod() {
    let store = Store::in_memory().unwrap();
    let hosts = store.collection::<Host>();
    let sets = store.collection::<DaemonSet>();
    let pods = store.collection::<Pod>();
    let ctl = DaemonSetController::new(store.clone());

    hosts
        .create(host("n1", &["mock"], BTreeMap::new()))
        .unwrap();
    sets.create(Resource::new(
        "sys",
        None,
        DaemonSetSpec {
            selector: Default::default(),
            template: template(),
        },
    ))
    .unwrap();
    ctl.reconcile_once().await.unwrap();
    assert_eq!(pods.list(None).unwrap().len(), 1);

    hosts
        .create(host("n2", &["mock"], BTreeMap::new()))
        .unwrap();
    ctl.reconcile_once().await.unwrap();
    assert_eq!(pods.list(None).unwrap().len(), 2);
}

#[tokio::test]
async fn http_crud_smoke() {
    use axum::{
        body::Body,
        http::{header, Request, StatusCode},
    };
    use http_body_util::BodyExt;
    use loom::Loom;
    use pistack_types::{kind, Caller, Id, Module, ModuleContext};
    use std::sync::Arc;
    use tokio_util::sync::CancellationToken;
    use tower::ServiceExt;

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
    let project = Id::new(kind::PROJECT);
    let caller = Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "t".into(),
        project: Some(project),
        roles: vec!["admin".into()],
        system: false,
    };
    let loom = Loom;
    let app = loom.router(ctx.clone()).layer(axum::middleware::from_fn(
        move |mut req: Request<Body>, next: axum::middleware::Next| {
            let caller = caller.clone();
            async move {
                req.extensions_mut().insert(caller);
                next.run(req).await
            }
        },
    ));
    loom.start(ctx).await.unwrap();

    let body = serde_json::json!({
        "name": "logger",
        "spec": {
            "template": {
                "labels": {"app": "logger"},
                "spec": {"containers": [{"name": "c", "image": "busybox"}]}
            }
        }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/daemonsets")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let ds: DaemonSet = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(ds.meta.name, "logger");
    assert_eq!(ds.spec.selector.match_labels.get("app").unwrap(), "logger");

    let body = serde_json::json!({
        "name": "db",
        "spec": {
            "replicas": 2,
            "service_name": "db",
            "template": {
                "labels": {"app": "db"},
                "spec": {"containers": [{"name": "c", "image": "postgres"}]}
            }
        }
    });
    let req = Request::builder()
        .method("POST")
        .uri("/statefulsets")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::CREATED);
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let sts: StatefulSet = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(sts.spec.replicas, 2);
    assert_eq!(sts.spec.service_name, "db");

    let req = Request::builder()
        .method("POST")
        .uri(format!("/statefulsets/{}/scale", sts.meta.id))
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(r#"{"replicas":1}"#))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);

    let req = Request::builder()
        .method("GET")
        .uri("/")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    let bytes = resp.into_body().collect().await.unwrap().to_bytes();
    let idx: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let resources = idx["resources"].as_array().unwrap();
    assert!(resources.iter().any(|r| r == "statefulsets"));
    assert!(resources.iter().any(|r| r == "daemonsets"));

    shutdown.cancel();
}
