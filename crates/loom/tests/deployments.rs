// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End to end: loom's controllers on top of forge's reconciler with the
//! mock driver. A Deployment becomes pods, pods become machines, machines
//! come up with addresses, a Service sees the endpoints, and everything
//! tears down cleanly.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use http_body_util::BodyExt;
use loom::{
    ConfigMap, CronJob, Deployment, Ingress, Job, Loom, Pod, PodPhase, ReplicaSet, Service,
    MACHINE_POD_LABEL, POD_TEMPLATE_HASH,
};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Meta, Module, ModuleContext, Page, Phase};
use serde::de::DeserializeOwned;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

struct Harness {
    app: Router,
    store: Store,
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
    };
    forge::Forge::default().start(ctx.clone()).await.unwrap();
    // The dev host advertises this box's cores; give the tests room.
    let hosts = store.collection::<forge::Host>();
    let local = hosts.get_by_name(None, forge::LOCAL_HOST_NAME).unwrap();
    hosts
        .modify(&local.meta.id, |h| {
            h.spec.capacity = pistack_types::Resources {
                vcpus: 64,
                memory_mib: 262_144,
                disk_gib: 10_000,
                gpus: 0,
            };
            // A machine that names a network is placed on a host that has a
            // private underlay address.
            h.spec.underlay_ip = Some("10.1.0.2".into());
            Ok(())
        })
        .unwrap();
    // Pod machines join the network named "default" and need a project
    // before a port can be allocated.
    let project = Id::new(kind::PROJECT);
    fabric::Client::new(store.clone(), Arc::new(fabric::MockBackend::new()))
        .create_network(
            Meta::new(kind::NETWORK, "default", None),
            fabric::NetworkSpec::default(),
        )
        .unwrap();
    let caller = Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "loom-test".into(),
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
    Harness {
        app,
        store,
        shutdown,
    }
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

/// Poll until `pred` holds or 10 s pass.
async fn wait_until<F: FnMut() -> Option<String>>(what: &str, mut pred: F) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        match pred() {
            None => return,
            Some(last) => {
                assert!(
                    tokio::time::Instant::now() < deadline,
                    "timed out waiting for {what}; last: {last}"
                );
            }
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn web_template() -> serde_json::Value {
    serde_json::json!({
        "labels": {"app": "web"},
        "spec": {
            "containers": [{
                "name": "web",
                "image": "nginx:1.27",
                "ports": [{"container_port": 80}],
                "resources": {"vcpus": 1, "memory_mib": 512}
            }]
        }
    })
}

#[tokio::test]
async fn deployment_to_running_pods_and_back() {
    let h = harness().await;
    let app = &h.app;
    let pods = h.store.collection::<Pod>();
    let sets = h.store.collection::<ReplicaSet>();
    let machines = h.store.collection::<forge::Machine>();

    // Deployment with three replicas.
    let (st, d) = call::<Deployment>(
        app,
        "POST",
        "/deployments",
        Some(serde_json::json!({
            "name": "web",
            "spec": {"replicas": 3, "template": web_template()}
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{d:?}");
    // Selector defaulted from the template labels.
    assert_eq!(d.spec.selector.match_labels.get("app").unwrap(), "web");
    let did = d.meta.id.to_string();

    // Service selecting them.
    let (st, svc) = call::<Service>(
        app,
        "POST",
        "/services",
        Some(serde_json::json!({
            "name": "web",
            "spec": {"selector": {"app": "web"}, "ports": [{"port": 80}]}
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{svc:?}");
    let sid = svc.meta.id.to_string();

    // One ReplicaSet with the template hash, three pods.
    wait_until("3 pods", || {
        let n = pods.list(None).unwrap().len();
        (n != 3).then(|| format!("{n} pods"))
    })
    .await;
    let rs_list = sets.list(None).unwrap();
    assert_eq!(rs_list.len(), 1);
    let rs = &rs_list[0];
    assert!(rs.meta.name.starts_with("web-"));
    let hash = rs.meta.labels.get(POD_TEMPLATE_HASH).unwrap().clone();
    for p in pods.list(None).unwrap() {
        assert!(
            p.meta.name.starts_with(&format!("{}-", rs.meta.name)),
            "{}",
            p.meta.name
        );
        assert_eq!(p.meta.labels.get(POD_TEMPLATE_HASH), Some(&hash));
        assert_eq!(p.meta.labels.get("app").unwrap(), "web");
    }

    // Machines of kind container, one per pod, carrying the sandbox document.
    wait_until("3 machines", || {
        let n = machines.list(None).unwrap().len();
        (n != 3).then(|| format!("{n} machines"))
    })
    .await;
    for m in machines.list(None).unwrap() {
        assert_eq!(m.spec.kind, forge::MachineKind::Container);
        assert_eq!(m.spec.image, "nginx:1.27");
        assert_eq!(m.spec.networks, vec!["default"]);
        assert_eq!(m.spec.size, "s.1x1", "smallest fitting seeded size");
        assert!(m.meta.labels.contains_key(MACHINE_POD_LABEL));
        let sandbox: loom::PodSandbox =
            serde_json::from_str(m.spec.user_data.as_deref().unwrap()).unwrap();
        assert_eq!(sandbox.kind, loom::SANDBOX_KIND);
        assert_eq!(sandbox.containers[0].name, "web");
        assert_eq!(
            sandbox.pod.id.to_string(),
            *m.meta.labels.get(MACHINE_POD_LABEL).unwrap()
        );
    }

    // Pods go Running with the mock's addresses mirrored in.
    wait_until("pods running", || {
        let ps = pods.list(None).unwrap();
        let running = ps
            .iter()
            .filter(|p| p.status.phase == PodPhase::Running && p.status.pod_ip.is_some())
            .count();
        (running != 3).then(|| {
            format!(
                "{running}/3 running: {:?}",
                ps.iter()
                    .map(|p| (&p.status.phase, &p.status.message))
                    .collect::<Vec<_>>()
            )
        })
    })
    .await;
    let mut ips: Vec<String> = pods
        .list(None)
        .unwrap()
        .into_iter()
        .map(|p| p.status.pod_ip.unwrap())
        .collect();
    ips.sort();
    assert_eq!(ips, vec!["10.0.0.1", "10.0.0.2", "10.0.0.3"]);
    for p in pods.list(None).unwrap() {
        assert!(p.status.host.is_some());
        assert!(p.status.container_statuses[0].ready);
    }

    // Deployment status catches up.
    wait_until("deployment available", || {
        let d = h.store.collection::<Deployment>().get(&d.meta.id).unwrap();
        (d.status.available != 3 || d.status.updated != 3 || d.status.revision != 1)
            .then(|| format!("{:?}", d.status))
    })
    .await;

    // Service: cluster IP from the default CIDR, three endpoints.
    wait_until("3 endpoints", || {
        let s = h.store.collection::<Service>().get(&svc.meta.id).unwrap();
        (s.status.endpoints.len() != 3 || s.status.cluster_ip.is_none())
            .then(|| format!("{:?}", s.status))
    })
    .await;
    let (_, svc) = call::<Service>(app, "GET", &format!("/services/{sid}"), None).await;
    let ip: std::net::Ipv4Addr = svc.status.cluster_ip.as_deref().unwrap().parse().unwrap();
    assert_eq!(ip.octets()[0], 10);
    assert!((96..112).contains(&ip.octets()[1]), "{ip}");
    let mut ep: Vec<&str> = svc.status.endpoints.iter().map(|e| e.ip.as_str()).collect();
    ep.sort();
    assert_eq!(ep, vec!["10.0.0.1", "10.0.0.2", "10.0.0.3"]);

    // List filters and the logs stub.
    let (_, page) = call::<Page<Pod>>(app, "GET", "/pods?labels=app%3Dweb", None).await;
    assert_eq!(page.total, 3);
    let (_, page) = call::<Page<Pod>>(app, "GET", "/pods?labels=app%3Ddb", None).await;
    assert_eq!(page.total, 0);
    let pid = page_first_id(&pods);
    let (st, _) = raw(app, "GET", &format!("/pods/{pid}/logs"), None).await;
    assert_eq!(st, StatusCode::SERVICE_UNAVAILABLE);

    // Scale to one: two pods and their machines go away; one endpoint remains.
    let (st, d) = call::<Deployment>(
        app,
        "POST",
        &format!("/deployments/{did}/scale"),
        Some(serde_json::json!({"replicas": 1})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(d.spec.replicas, 1);
    wait_until("scale to 1", || {
        let np = pods.list(None).unwrap().len();
        let nm = machines.list(None).unwrap().len();
        let ne = h
            .store
            .collection::<Service>()
            .get(&svc.meta.id)
            .unwrap()
            .status
            .endpoints
            .len();
        (np != 1 || nm != 1 || ne != 1).then(|| format!("{np} pods, {nm} machines, {ne} endpoints"))
    })
    .await;
    let (_, rs) =
        call::<ReplicaSet>(app, "GET", &format!("/replicasets/{}", rs.meta.id), None).await;
    assert_eq!(rs.spec.replicas, 1);
    assert_eq!(rs.status.replicas, 1);

    // Delete the deployment: accepted, then the whole tree is gone.
    let (st, _) = raw(app, "DELETE", &format!("/deployments/{did}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    wait_until("teardown", || {
        let np = pods.list(None).unwrap().len();
        let nm = machines.list(None).unwrap().len();
        let nr = sets.list(None).unwrap().len();
        let nd = h.store.collection::<Deployment>().list(None).unwrap().len();
        (np + nm + nr + nd != 0).then(|| format!("{np} pods, {nm} machines, {nr} rs, {nd} deploy"))
    })
    .await;
    let (st, _) = raw(app, "GET", &format!("/deployments/{did}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    // The service survives with no endpoints.
    let (_, svc) = call::<Service>(app, "GET", &format!("/services/{sid}"), None).await;
    assert!(svc.status.endpoints.is_empty());
    // And deletes through its controller.
    let (st, _) = raw(app, "DELETE", &format!("/services/{sid}"), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    wait_until("service gone", || {
        let n = h.store.collection::<Service>().list(None).unwrap().len();
        (n != 0).then(|| format!("{n} services"))
    })
    .await;
}

fn page_first_id(pods: &pistack_store::Collection<Pod>) -> String {
    pods.list(None).unwrap()[0].meta.id.to_string()
}

#[tokio::test]
async fn rolling_update_and_rollback() {
    let h = harness().await;
    let app = &h.app;
    let pods = h.store.collection::<Pod>();
    let sets = h.store.collection::<ReplicaSet>();

    let (st, d) = call::<Deployment>(
        app,
        "POST",
        "/deployments",
        Some(serde_json::json!({
            "name": "api",
            "spec": {
                "replicas": 2,
                "template": web_template(),
                "strategy": {"type": "RollingUpdate", "max_surge": 1, "max_unavailable": 0}
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{d:?}");
    let did = d.meta.id.to_string();
    wait_until("v1 available", || {
        let d = h.store.collection::<Deployment>().get(&d.meta.id).unwrap();
        (d.status.available != 2).then(|| format!("{:?}", d.status))
    })
    .await;
    let v1_hash = sets.list(None).unwrap()[0]
        .meta
        .labels
        .get(POD_TEMPLATE_HASH)
        .unwrap()
        .clone();

    // New image: a second ReplicaSet, rolled with at most 3 pods at a time.
    let mut t2 = web_template();
    t2["spec"]["containers"][0]["image"] = serde_json::json!("nginx:1.28");
    let mut spec = serde_json::to_value(&d.spec).unwrap();
    spec["template"] = t2;
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/deployments/{did}"),
        Some(serde_json::json!({"spec": spec})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);

    let mut max_seen = 0usize;
    wait_until("v2 rolled out", || {
        let ps: Vec<Pod> = pods
            .list(None)
            .unwrap()
            .into_iter()
            .filter(|p| !loom::delete_requested(p))
            .collect();
        max_seen = max_seen.max(ps.len());
        let v2 = ps
            .iter()
            .filter(|p| {
                p.spec.containers[0].image == "nginx:1.28" && p.status.phase == PodPhase::Running
            })
            .count();
        let v1 = ps
            .iter()
            .filter(|p| p.spec.containers[0].image == "nginx:1.27")
            .count();
        (v2 != 2 || v1 != 0).then(|| format!("v1={v1} v2={v2}"))
    })
    .await;
    assert!(max_seen <= 3, "surge exceeded: saw {max_seen} pods");
    let rs_list = sets.list(None).unwrap();
    assert_eq!(rs_list.len(), 2, "old set kept for rollback");
    let old = rs_list
        .iter()
        .find(|rs| rs.meta.labels.get(POD_TEMPLATE_HASH) == Some(&v1_hash))
        .unwrap();
    assert_eq!(old.spec.replicas, 0);
    wait_until("revision 2", || {
        let d = h.store.collection::<Deployment>().get(&d.meta.id).unwrap();
        (d.status.revision != 2 || d.status.updated != 2).then(|| format!("{:?}", d.status))
    })
    .await;

    // Rollback goes back to the v1 template and re-activates its set.
    let (st, d) =
        call::<Deployment>(app, "POST", &format!("/deployments/{did}/rollback"), None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(d.spec.template.spec.containers[0].image, "nginx:1.27");
    assert!(!d.spec.template.labels.contains_key(POD_TEMPLATE_HASH));
    wait_until("rolled back", || {
        let ps = pods.list(None).unwrap();
        let v1 = ps
            .iter()
            .filter(|p| {
                p.spec.containers[0].image == "nginx:1.27" && p.status.phase == PodPhase::Running
            })
            .count();
        let d = h.store.collection::<Deployment>().get(&d.meta.id).unwrap();
        (v1 != 2 || ps.len() != 2 || d.status.revision != 3)
            .then(|| format!("v1={v1} n={} rev={}", ps.len(), d.status.revision))
    })
    .await;
    assert_eq!(sets.list(None).unwrap().len(), 2);
    let (st, _) = raw(
        app,
        "POST",
        &format!("/deployments/{did}/rollback"),
        Some(serde_json::json!({"revision": 99})),
    )
    .await;
    assert_eq!(st, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn crud_validation_and_conflicts() {
    let h = harness().await;
    let app = &h.app;

    // ConfigMap: create, list, stale If-Match conflicts, immutable, delete.
    let (st, cm) = call::<ConfigMap>(
        app,
        "POST",
        "/configmaps",
        Some(serde_json::json!({"name": "cfg", "spec": {"data": {"a": "1"}}})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let cid = cm.meta.id.to_string();
    let (st, _) = raw(
        app,
        "POST",
        "/configmaps",
        Some(serde_json::json!({"name": "cfg", "spec": {}})),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, cm2) = call::<ConfigMap>(
        app,
        "PUT",
        &format!("/configmaps/{cid}"),
        Some(serde_json::json!({"spec": {"data": {"a": "2"}, "immutable": true}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(cm2.meta.revision, 2);
    let req = Request::builder()
        .method("PUT")
        .uri(format!("/configmaps/{cid}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::IF_MATCH, "1")
        .body(Body::from(
            serde_json::to_vec(&serde_json::json!({"description": "x"})).unwrap(),
        ))
        .unwrap();
    assert_eq!(
        app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::CONFLICT
    );
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/configmaps/{cid}"),
        Some(serde_json::json!({"spec": {"data": {"a": "3"}, "immutable": true}})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "immutable");
    let (_, page) = call::<Page<ConfigMap>>(app, "GET", "/configmaps?name=cfg", None).await;
    assert_eq!(page.total, 1);
    let (st, _) = raw(app, "GET", "/configmaps/cfg", None).await;
    assert_eq!(st, StatusCode::OK, "resolve by name");
    let (st, _) = raw(app, "DELETE", &format!("/configmaps/{cid}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = raw(app, "GET", &format!("/configmaps/{cid}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    // Validation.
    for (path, body, why) in [
        (
            "/pods",
            serde_json::json!({"name": "Bad", "spec": {"containers": []}}),
            "name",
        ),
        (
            "/pods",
            serde_json::json!({"name": "p", "spec": {"containers": []}}),
            "no containers",
        ),
        (
            "/pods",
            serde_json::json!({"name": "p", "spec": {"containers": [{"name": "c", "image": ""}]}}),
            "no image",
        ),
        (
            "/deployments",
            serde_json::json!({"name": "d", "spec": {"selector": {"match_labels": {"app": "x"}}, "template": web_template()}}),
            "selector mismatch",
        ),
        (
            "/jobs",
            serde_json::json!({"name": "j", "spec": {"template": web_template()}}),
            "job with restart Always",
        ),
        (
            "/cronjobs",
            serde_json::json!({"name": "c", "spec": {"schedule": "bogus", "job_template": {"template": web_template()}}}),
            "bad schedule",
        ),
        (
            "/services",
            serde_json::json!({"name": "s", "spec": {"selector": {"app": "x"}, "ports": []}}),
            "no ports",
        ),
        (
            "/ingresses",
            serde_json::json!({"name": "i", "spec": {}}),
            "no rules",
        ),
    ] {
        let (st, bytes) = raw(app, "POST", path, Some(body)).await;
        assert_eq!(
            st,
            StatusCode::BAD_REQUEST,
            "{why}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }

    // Ingress is stored as-is.
    let (st, ing) = call::<Ingress>(
        app,
        "POST",
        "/ingresses",
        Some(serde_json::json!({
            "name": "web",
            "spec": {
                "rules": [{"host": "web.example.com", "paths": [{"path": "/", "backend": {"service": "web", "port": 80}}]}],
                "tls": [{"hosts": ["web.example.com"], "secret_name": "web-tls"}]
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{ing:?}");
    assert_eq!(ing.spec.rules[0].host.as_deref(), Some("web.example.com"));
    let (st, _) = raw(app, "DELETE", &format!("/ingresses/{}", ing.meta.id), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);

    // CronJob stores and schedules.
    let mut jt = web_template();
    jt["spec"]["restart_policy"] = serde_json::json!("Never");
    let (st, cron) = call::<CronJob>(
        app,
        "POST",
        "/cronjobs",
        Some(serde_json::json!({
            "name": "nightly",
            "spec": {"schedule": "0 3 * * *", "job_template": {"template": jt}}
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{cron:?}");
    wait_until("cron scheduled", || {
        let c = h.store.collection::<CronJob>().get(&cron.meta.id).unwrap();
        c.status
            .next_schedule_time
            .is_none()
            .then(|| format!("{:?}", c.status))
    })
    .await;
    let c = h.store.collection::<CronJob>().get(&cron.meta.id).unwrap();
    let next = c.status.next_schedule_time.unwrap();
    assert_eq!((next.timestamp() / 3600) % 24, 3, "{next}");
    assert!(h.store.collection::<Job>().list(None).unwrap().is_empty());
    let (st, _) = raw(app, "DELETE", &format!("/cronjobs/{}", cron.meta.id), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    wait_until("cron gone", || {
        let n = h.store.collection::<CronJob>().list(None).unwrap().len();
        (n != 0).then(|| format!("{n}"))
    })
    .await;
}

#[tokio::test]
async fn pod_with_odd_size_gets_an_exact_size() {
    let h = harness().await;
    let app = &h.app;
    let (st, p) = call::<Pod>(
        app,
        "POST",
        "/pods",
        Some(serde_json::json!({
            "name": "big",
            "spec": {"containers": [
                {"name": "a", "image": "x", "resources": {"vcpus": 9, "memory_mib": 3000}},
                {"name": "b", "image": "y", "resources": {"vcpus": 9, "memory_mib": 3000}}
            ]}
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{p:?}");
    wait_until("pod running", || {
        let p = h.store.collection::<Pod>().get(&p.meta.id).unwrap();
        (p.status.phase != PodPhase::Running).then(|| format!("{:?}", p.status))
    })
    .await;
    let p = h.store.collection::<Pod>().get(&p.meta.id).unwrap();
    assert_eq!(p.status.container_statuses.len(), 2);
    let m = h
        .store
        .collection::<forge::Machine>()
        .get(p.status.machine.as_ref().unwrap())
        .unwrap();
    assert_eq!(m.spec.size, "pod-18x6000");
    assert_eq!(m.status.phase, Phase::Ready);
    let size = h
        .store
        .collection::<forge::Size>()
        .get_by_name(None, "pod-18x6000")
        .unwrap();
    assert_eq!(size.spec.resources.vcpus, 18);
    // Spec is immutable once the machine exists; labels are not.
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/pods/{}", p.meta.id),
        Some(serde_json::json!({"spec": {"containers": [{"name": "a", "image": "z"}]}})),
    )
    .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY);
    let (st, _) = raw(
        app,
        "PUT",
        &format!("/pods/{}", p.meta.id),
        Some(serde_json::json!({"labels": {"x": "y"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    // Delete: machine goes first, then the pod.
    let (st, _) = raw(app, "DELETE", &format!("/pods/{}", p.meta.id), None).await;
    assert_eq!(st, StatusCode::ACCEPTED);
    wait_until("pod and machine gone", || {
        let np = h.store.collection::<Pod>().list(None).unwrap().len();
        let nm = h
            .store
            .collection::<forge::Machine>()
            .list(None)
            .unwrap()
            .len();
        (np + nm != 0).then(|| format!("{np} pods {nm} machines"))
    })
    .await;
}
