// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Host maintenance through the router: an admin labels, disables, enables,
//! drains and evacuates hosts, and the scheduler follows.

use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use forge::maintain::{DrainReport, EvacuateReport};
use forge::{
    new_host, DriverRegistry, Forge, Host, HostDriver, Machine, MachineRuntime, MockDriver,
    EVACUATED_FROM_ANNOTATION,
};
use http_body_util::BodyExt;
use pistack_rpc::proto::{CommandKind, CommandResult, Heartbeat, MachineReport, MigrateRequest};
use pistack_rpc::{CommandQueue, QueueConfig};
use pistack_store::Store;
use pistack_types::{
    kind, ApiError, ApiResult, Caller, Id, Module, ModuleContext, Phase, Resource, Resources, Spec,
};
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
    harness_on(None).await
}

/// The same, with the agent transport when `queue` is given.
async fn harness_on(queue: Option<CommandQueue>) -> Harness {
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
    let forge = match queue {
        Some(queue) => Forge::with_queue(drivers.clone(), queue),
        None => Forge::with_registry(drivers.clone()),
    };
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
        self.add_host_with(name, &["mock"], None)
    }

    /// The same, advertising `drivers` and an underlay address. The mock
    /// still does the work, so a host can claim libvirt.
    fn add_host_with(
        &self,
        name: &str,
        drivers: &[&str],
        underlay: Option<&str>,
    ) -> (Host, Arc<MockDriver>) {
        let mut labels = BTreeMap::new();
        labels.insert("pistack.io/agent".to_owned(), "true".to_owned());
        let mut host = new_host(
            name,
            name,
            Resources {
                vcpus: 8,
                memory_mib: 16384,
                disk_gib: 200,
                gpus: 0,
            },
            drivers.iter().map(|d| d.to_string()).collect(),
            labels,
        );
        host.spec.underlay_ip = underlay.map(str::to_owned);
        let host = self.store.collection::<Host>().create(host).unwrap();
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

fn names(left: &[forge::maintain::Left]) -> Vec<(&str, &str)> {
    left.iter()
        .map(|l| (l.name.as_str(), l.reason.as_str()))
        .collect()
}

#[tokio::test]
async fn drain_migrates_what_it_can_and_reports_the_rest() {
    let queue = CommandQueue::new(QueueConfig {
        command_timeout: Duration::from_secs(30),
        ..QueueConfig::default()
    });
    let h = harness_on(Some(queue.clone())).await;
    let vm_host = ["kvm", "libvirt", "youki"];
    let (src, src_driver) = h.add_host_with("src", &vm_host, Some("10.1.0.1"));
    let (dst, _) = h.add_host_with("dst", &vm_host, Some("10.1.0.2"));

    // Everything starts on src: dst is closed while the machines are made.
    let (st, _) = raw(&h.app, &admin(), "POST", "/hosts/dst/disable", None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, _) = raw(
        &h.app,
        &admin(),
        "PUT",
        "/hosts/src/labels",
        Some(json!({"labels": {"rack": "r1"}})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let web = h.machine("web", vm(json!({}))).await;
    let off = h.machine("off", vm(json!({}))).await;
    let pod = h.machine("pod", vm(json!({"kind": "container"}))).await;
    let tied = h
        .machine("tied", vm(json!({"host_selector": {"rack": "r1"}})))
        .await;
    for m in [&web, &off, &pod, &tied] {
        assert_eq!(h.ready_on(&m.meta.id).await, src.meta.id);
    }
    let (st, _) = raw(
        &h.app,
        &admin(),
        "POST",
        &format!("/machines/{}/stop", off.meta.id),
        None,
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    h.wait(&off.meta.id, |m| m.status.power == forge::PowerState::Off)
        .await;
    let (st, _) = raw(&h.app, &admin(), "POST", "/hosts/dst/enable", None).await;
    assert_eq!(st, StatusCode::OK);

    let (st, _) = raw(&h.app, &project_admin(), "POST", "/hosts/src/drain", None).await;
    assert_eq!(st, StatusCode::FORBIDDEN);
    assert_eq!(queue.pending_count(), 0);

    let (st, report) =
        call::<DrainReport>(&h.app, &admin(), "POST", "/hosts/src/drain", None).await;
    assert_eq!(st, StatusCode::OK);
    // Draining disables the host and says why.
    assert!(report.host.spec.disabled);
    assert_eq!(report.host.spec.disabled_reason, "draining");
    // The running VM is on its way to the other host.
    assert_eq!(report.migrating.len(), 1, "{report:?}");
    assert_eq!(report.migrating[0].id, web.meta.id);
    assert_eq!(report.migrating[0].to, dst.meta.id);
    assert_eq!(report.migrating[0].to_name, "dst");
    // The rest are named with the reason, in id order.
    let left = names(&report.not_moved);
    assert_eq!(left.len(), 3, "{left:?}");
    assert_eq!(left[0].0, "off");
    assert!(left[0].1.contains("powered off"), "{left:?}");
    assert_eq!(left[1].0, "pod");
    assert!(left[1].1.contains("container"), "{left:?}");
    assert_eq!(left[2].0, "tied");
    assert!(left[2].1.contains("selector"), "{left:?}");

    // One migrate command went to the source host's agent.
    let cmds = queue.next("src", Duration::from_secs(1), 8).await;
    assert_eq!(cmds.len(), 1);
    assert_eq!(cmds[0].kind, CommandKind::MigrateMachine);
    let req: MigrateRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
    assert_eq!(req.id, web.meta.id);
    assert_eq!(req.dest, "qemu+ssh://10.1.0.2/system");

    // Running it again starts nothing new and gives the same account.
    let (st, again) = call::<DrainReport>(&h.app, &admin(), "POST", "/hosts/src/drain", None).await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(again.migrating, report.migrating);
    assert_eq!(again.not_moved, report.not_moved);
    assert!(queue
        .next("src", Duration::from_millis(100), 8)
        .await
        .is_empty());

    // The agent finishes and the destination reports the machine.
    queue.ack("src", CommandResult::ok(cmds[0].id)).unwrap();
    forge::Registrar::new(h.store.clone(), h.drivers.clone(), queue.clone())
        .on_heartbeat(&Heartbeat {
            node_name: "dst".into(),
            resources_free: Default::default(),
            machines: vec![MachineReport {
                id: web.meta.id.clone(),
                phase: Phase::Ready,
                message: String::new(),
                resources: None,
                addresses: vec![],
                power: Some(pistack_rpc::proto::PowerState::On),
                probes: vec![],
            }],
            at: chrono::Utc::now(),
            ovn: None,
            usage: None,
        })
        .unwrap();
    let moved = h
        .wait(&web.meta.id, |m| m.status.phase == Phase::Ready)
        .await;
    assert_eq!(moved.status.host.as_ref(), Some(&dst.meta.id));

    // A third run has nothing to migrate. What could not move was not
    // stopped: it is still on the host, in the state it was in.
    let (_, last) = call::<DrainReport>(&h.app, &admin(), "POST", "/hosts/src/drain", None).await;
    assert!(last.migrating.is_empty(), "{last:?}");
    assert_eq!(last.not_moved, report.not_moved);
    for m in [&pod, &tied] {
        let m = h.store.collection::<Machine>().get(&m.meta.id).unwrap();
        assert_eq!(m.status.phase, Phase::Ready);
        assert_eq!(m.status.power, forge::PowerState::On);
        assert_eq!(m.status.host.as_ref(), Some(&src.meta.id));
    }
    assert_eq!(src_driver.len(), 4);
    // A reason given later replaces the default one.
    let (_, named) = call::<DrainReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/src/drain",
        Some(json!({"reason": "kernel update"})),
    )
    .await;
    assert_eq!(named.host.spec.disabled_reason, "kernel update");
}

#[tokio::test]
async fn drain_without_live_migration_moves_nothing_and_says_so() {
    // Dev mode shape: mock hosts and no agent transport.
    let h = harness().await;
    let (a, a_driver) = h.add_host("a");
    let m = h.machine("only", vm(json!({}))).await;
    assert_eq!(h.ready_on(&m.meta.id).await, a.meta.id);
    h.add_host("b");

    let (st, report) = call::<DrainReport>(&h.app, &admin(), "POST", "/hosts/a/drain", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(report.host.spec.disabled);
    assert!(report.migrating.is_empty());
    let left = names(&report.not_moved);
    assert_eq!(left.len(), 1);
    assert!(left[0].1.contains("libvirt"), "{left:?}");
    let m = h.store.collection::<Machine>().get(&m.meta.id).unwrap();
    assert_eq!(m.status.phase, Phase::Ready);
    assert_eq!(m.status.host.as_ref(), Some(&a.meta.id));
    assert_eq!(a_driver.len(), 1);
}

// slab's rows as forge reads them. forge does not depend on slab.

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct VolumeSpec {
    volume_type: String,
}

impl Spec for VolumeSpec {
    const KIND: &'static str = kind::VOLUME;
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct VolumeStatus {
    attached_to: Option<Id>,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct VolumeTypeSpec {
    backend: String,
}

impl Spec for VolumeTypeSpec {
    const KIND: &'static str = "voltype";
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
struct Nothing {}

impl Harness {
    fn volume_type(&self, name: &str, backend: &str) {
        self.store
            .collection::<Resource<VolumeTypeSpec, Nothing>>()
            .create(Resource::new(
                name,
                None,
                VolumeTypeSpec {
                    backend: backend.into(),
                },
            ))
            .unwrap();
    }

    /// A volume of `volume_type` recorded as attached to `machine`.
    fn attach(&self, name: &str, volume_type: &str, machine: &Id) {
        let mut vol = Resource::<VolumeSpec, VolumeStatus>::new(
            name,
            None,
            VolumeSpec {
                volume_type: volume_type.into(),
            },
        );
        vol.status.attached_to = Some(machine.clone());
        self.store
            .collection::<Resource<VolumeSpec, VolumeStatus>>()
            .create(vol)
            .unwrap();
    }

    /// Make the reconciler see `host` as down: no report for a minute.
    async fn go_silent(&self, host: &Id) {
        self.drivers.unregister(host);
        let driver: Arc<dyn HostDriver> = Arc::new(Unreachable);
        self.drivers.register(host.clone(), driver);
        self.store
            .collection::<Host>()
            .modify(host, |h| {
                h.status.last_seen = Some(chrono::Utc::now() - chrono::Duration::seconds(60));
                Ok(())
            })
            .unwrap();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while self
            .store
            .collection::<Host>()
            .get(host)
            .unwrap()
            .status
            .phase
            != Phase::Error
        {
            assert!(
                tokio::time::Instant::now() < deadline,
                "host never went down"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

/// The driver of a host nothing answers on.
struct Unreachable;

#[async_trait::async_trait]
impl HostDriver for Unreachable {
    async fn start(&self, _m: &Machine) -> ApiResult<MachineRuntime> {
        Err(ApiError::unavailable("host does not answer"))
    }
    async fn stop(&self, _id: &Id) -> ApiResult<()> {
        Err(ApiError::unavailable("host does not answer"))
    }
    async fn delete(&self, _id: &Id) -> ApiResult<()> {
        Err(ApiError::unavailable("host does not answer"))
    }
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Err(ApiError::unavailable("host does not answer"))
    }
}

fn evacuate_body(confirm_dead: bool, rebuild: bool) -> Option<serde_json::Value> {
    Some(json!({"confirm_dead": confirm_dead, "rebuild_from_image": rebuild}))
}

#[tokio::test]
async fn evacuate_reschedules_a_down_host_and_skips_what_cannot_follow() {
    let h = harness().await;
    let (dead, _) = h.add_host("dead");
    let (spare, spare_driver) = h.add_host("spare");
    h.volume_type("standard", "local");
    h.volume_type("ceph", "rbd");

    // Everything starts on `dead`.
    let (st, _) = raw(&h.app, &admin(), "POST", "/hosts/spare/disable", None).await;
    assert_eq!(st, StatusCode::OK);
    let plain = h.machine("plain", vm(json!({}))).await;
    let shared = h.machine("shared", vm(json!({}))).await;
    let local = h.machine("local", vm(json!({}))).await;
    let unknown = h.machine("unknown", vm(json!({}))).await;
    let metal = h.machine("metal", vm(json!({"kind": "metal"}))).await;
    for m in [&plain, &shared, &local, &unknown, &metal] {
        assert_eq!(h.ready_on(&m.meta.id).await, dead.meta.id);
    }
    h.attach("blocks", "ceph", &shared.meta.id);
    h.attach("scratch", "standard", &local.meta.id);
    h.attach("blocks2", "ceph", &local.meta.id);
    h.attach("odd", "gone-type", &unknown.meta.id);
    let (st, _) = raw(&h.app, &admin(), "POST", "/hosts/spare/enable", None).await;
    assert_eq!(st, StatusCode::OK);

    // A host that is up is refused, whatever the flags say, and so is a
    // project admin.
    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        evacuate_body(true, true),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let (st, _) = raw(
        &h.app,
        &project_admin(),
        "POST",
        "/hosts/dead/evacuate",
        evacuate_body(true, true),
    )
    .await;
    assert_eq!(st, StatusCode::FORBIDDEN);

    h.go_silent(&dead.meta.id).await;

    // Without the flag nothing moves: every machine would lose its disk.
    let (st, report) =
        call::<EvacuateReport>(&h.app, &admin(), "POST", "/hosts/dead/evacuate", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(report.evacuated.is_empty(), "{report:?}");
    assert_eq!(report.skipped.len(), 5);
    let why = names(&report.skipped);
    assert!(why[0].1.contains("root disk"), "{why:?}");
    assert!(why[0].1.contains("rebuild_from_image"), "{why:?}");
    for m in [&plain, &shared] {
        let m = h.store.collection::<Machine>().get(&m.meta.id).unwrap();
        assert_eq!(m.status.host.as_ref(), Some(&dead.meta.id));
    }

    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        evacuate_body(false, true),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let moved: Vec<&str> = report.evacuated.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(moved, ["plain", "shared"]);
    assert_eq!(report.evacuated[0].rebuilt_from, "ubuntu");
    assert!(report.evacuated[0].volumes.is_empty());
    // The shared volume is named: it is plugged back in.
    assert_eq!(report.evacuated[1].volumes, ["blocks"]);
    let why = names(&report.skipped);
    assert_eq!(why.len(), 3, "{why:?}");
    assert_eq!(why[0].0, "local");
    assert!(why[0].1.contains("volume scratch"), "{why:?}");
    assert!(why[0].1.contains("local storage"), "{why:?}");
    assert_eq!(why[1].0, "unknown");
    assert!(why[1].1.contains("volume odd"), "{why:?}");
    assert_eq!(why[2].0, "metal");
    assert!(why[2].1.contains("bare metal"), "{why:?}");

    // The evacuated machines come up on the other host and say what happened.
    for m in [&plain, &shared] {
        assert_eq!(h.ready_on(&m.meta.id).await, spare.meta.id);
        let m = h.store.collection::<Machine>().get(&m.meta.id).unwrap();
        assert_eq!(
            m.meta.annotations.get(EVACUATED_FROM_ANNOTATION),
            Some(&dead.meta.id.to_string())
        );
        let note = m
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Evacuated")
            .expect("an Evacuated condition");
        assert!(note.status);
        assert!(note.message.contains("root disk"), "{}", note.message);
        assert!(note.message.contains("image ubuntu"), "{}", note.message);
    }
    assert_eq!(spare_driver.len(), 2);
    // The skipped ones were not touched.
    for m in [&local, &unknown, &metal] {
        let m = h.store.collection::<Machine>().get(&m.meta.id).unwrap();
        assert_eq!(m.status.host.as_ref(), Some(&dead.meta.id));
        assert_eq!(m.status.phase, Phase::Ready);
        assert!(!m.meta.annotations.contains_key(EVACUATED_FROM_ANNOTATION));
    }

    // Running it again moves nothing more and names the same three.
    let (_, again) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        evacuate_body(false, true),
    )
    .await;
    assert!(again.evacuated.is_empty(), "{again:?}");
    assert_eq!(again.skipped, report.skipped);
}

/// A driver whose start hangs until the test lets it fail.
struct Stuck {
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
}

#[async_trait::async_trait]
impl HostDriver for Stuck {
    async fn start(&self, _m: &Machine) -> ApiResult<MachineRuntime> {
        self.entered.notify_one();
        self.release.notified().await;
        Err(ApiError::unavailable("host went away during the start"))
    }
    async fn stop(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn delete(&self, _id: &Id) -> ApiResult<()> {
        Ok(())
    }
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Ok(Vec::new())
    }
}

#[tokio::test]
async fn a_disabled_host_is_evacuated_only_when_confirmed_dead() {
    let h = harness().await;
    let (hung, _) = h.add_host("hung");
    let stuck = Arc::new(Stuck {
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
    });
    h.drivers.unregister(&hung.meta.id);
    h.drivers
        .register_local(hung.meta.id.clone(), stuck.clone());

    // The machine is placed on the host and its start never returns.
    let m = h.machine("caught", vm(json!({}))).await;
    stuck.entered.notified().await;
    let (spare, _) = h.add_host("spare");

    // The host still looks alive. Disabled alone is not enough.
    let (st, _) = raw(&h.app, &admin(), "POST", "/hosts/hung/disable", None).await;
    assert_eq!(st, StatusCode::OK);
    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/hung/evacuate",
        evacuate_body(false, true),
    )
    .await;
    assert_eq!(st, StatusCode::CONFLICT);
    assert!(
        String::from_utf8_lossy(&body).contains("confirm"),
        "{}",
        String::from_utf8_lossy(&body)
    );

    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/hung/evacuate",
        evacuate_body(true, true),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(report.evacuated.len(), 1, "{report:?}");

    // The old start fails only now. Its answer is about the host the
    // machine has left and must not land on the machine.
    stuck.release.notify_one();
    assert_eq!(h.ready_on(&m.meta.id).await, spare.meta.id);
    let m = h.store.collection::<Machine>().get(&m.meta.id).unwrap();
    assert!(m.status.message.is_empty(), "{}", m.status.message);
}

#[tokio::test]
async fn a_host_shows_when_its_agent_certificate_expires() {
    let h = harness().await;
    h.add_host("a");
    h.add_host("b");
    let keep = keep::Client::open(&h.store).unwrap();
    let serial = |pki: &keep::Pki| {
        keep::ca::serial_from_der(&keep::ca::pem_to_der(&pki.cert_pem).unwrap()).unwrap()
    };
    let first = serial(&keep.issue_client_cert("a").unwrap());
    let record = keep.managed_certificate(first).unwrap().unwrap();

    let (st, host) = call::<Host>(&h.app, &admin(), "GET", "/hosts/a", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(host.status.cert_expires.is_some());
    assert_eq!(host.status.cert_expires, record.status.not_after);
    // A host that never enrolled has none, and the field is left out.
    let (_, body) = raw(&h.app, &admin(), "GET", "/hosts/b", None).await;
    let body = String::from_utf8(body).unwrap();
    assert!(!body.contains("cert_expires"), "{body}");

    let (_, page) =
        call::<pistack_types::Page<Host>>(&h.app, &admin(), "GET", "/hosts", None).await;
    let of = |name: &str| {
        page.items
            .iter()
            .find(|x| x.meta.name == name)
            .unwrap()
            .status
            .cert_expires
    };
    assert_eq!(of("a"), record.status.not_after);
    assert_eq!(of("b"), None);

    // The date is read from keep each time and never written to the host.
    let stored = h.store.collection::<Host>().resolve(None, "a").unwrap();
    assert_eq!(stored.status.cert_expires, None);
    // Revoked with nothing newer: the host has no certificate to show.
    keep.revoke_certificate(&record.meta.id).unwrap();
    let (_, host) = call::<Host>(&h.app, &admin(), "GET", "/hosts/a", None).await;
    assert_eq!(host.status.cert_expires, None);
}
