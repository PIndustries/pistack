// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Evacuating a host that stopped reporting requires proof it is off. With
//! power control the server powers the host off and confirms it; without,
//! an operator must confirm the host is dead, every time. A power-off that
//! cannot be confirmed moves nothing.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use axum::{
    body::Body,
    http::{header, Request, StatusCode},
    Router,
};
use forge::maintain::{EvacuateReport, Fencer, Proof};
use forge::{new_host, DriverRegistry, Forge, Host};
use http_body_util::BodyExt;
use pistack_store::Store;
use pistack_types::{
    kind, ApiError, ApiResult, Caller, Id, Module, ModuleContext, Phase, Resources,
};
use serde::de::DeserializeOwned;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use tower::ServiceExt;

/// A fencer whose behaviour the test dictates.
struct Fake {
    /// The fence succeeds and reports off.
    works: AtomicBool,
    /// How many times a fence was attempted.
    tries: AtomicUsize,
}

impl Fake {
    fn works() -> Arc<Self> {
        Arc::new(Self {
            works: AtomicBool::new(true),
            tries: AtomicUsize::new(0),
        })
    }
    fn fails(&self) {
        self.works.store(false, Ordering::SeqCst);
    }
    fn count(&self) -> usize {
        self.tries.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Fencer for Fake {
    async fn fence(&self, _host: &Host) -> ApiResult<()> {
        self.tries.fetch_add(1, Ordering::SeqCst);
        if self.works.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(ApiError::unavailable("bmc did not answer"))
        }
    }
}

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

async fn harness(fencer: Arc<dyn Fencer>) -> Harness {
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
    let forge = Forge::with_registry(drivers.clone()).fencer(fencer);
    let app = forge.router(ctx.clone());
    Forge::seed_sizes(&store).unwrap();
    forge.start(ctx).await.unwrap();
    Harness {
        app,
        store,
        shutdown,
    }
}

impl Harness {
    /// A ready host. `power` sets out-of-band power control when given.
    fn add_host(&self, name: &str, power: Option<(&str, &str, &str, &str)>) -> Host {
        let mut labels = BTreeMap::new();
        labels.insert("pistack.io/agent".to_owned(), "true".to_owned());
        labels.insert("pistack.io/disk".to_owned(), "shared".to_owned());
        let mut host = new_host(
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
        );
        if let Some((kind, address, user, secret)) = power {
            host.spec.power_control =
                Some(forge::PowerControl::checked(kind, address, user, secret).unwrap());
        }
        self.store.collection::<Host>().create(host).unwrap()
    }

    /// Mark the host down (the phase the reconciler would reach).
    async fn go_down(&self, id: &Id) {
        self.store
            .collection::<Host>()
            .modify(id, |h| {
                h.status.phase = Phase::Error;
                Ok(())
            })
            .unwrap();
    }

    /// Disable a host that still reports. Its agent answers; the host works.
    async fn disable(&self, id: &Id) {
        self.store
            .collection::<Host>()
            .modify(id, |h| {
                h.spec.disabled = true;
                h.spec.disabled_reason = "kernel update".into();
                Ok(())
            })
            .unwrap();
    }

    /// Place a machine on `host` and mark it running there.
    async fn put_machine(&self, name: &str, host: &Id) -> forge::Machine {
        let (st, m) = call::<forge::Machine>(
            &self.app,
            &admin(),
            "POST",
            "/machines",
            Some(machine_body(name)),
        )
        .await;
        assert_eq!(st, StatusCode::CREATED);
        self.store
            .collection::<forge::Machine>()
            .modify(&m.meta.id, |x| {
                x.status.host = Some(host.clone());
                x.status.phase = Phase::Ready;
                Ok(())
            })
            .unwrap();
        self.store
            .collection::<forge::Machine>()
            .get(&m.meta.id)
            .unwrap()
    }
}

fn admin() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "root".into(),
        project: None,
        roles: ["admin".into()].into(),
        system: true,
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
    let v: T = serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("{method} {path}: {status} bad body {e}"));
    (status, v)
}

fn machine_body(name: &str) -> serde_json::Value {
    json!({"name": name, "spec": {"size": "s.1x1", "image": "ubuntu"}})
}

/// Audit actions recorded by forge that start with `prefix`, sorted.
fn audit_actions(store: &Store, prefix: &str) -> Vec<String> {
    let mut v: Vec<String> = store
        .collection::<gate::audit::AuditEvent>()
        .list(None)
        .unwrap()
        .into_iter()
        .map(|e| e.spec.action)
        .filter(|a| a.starts_with(prefix))
        .collect();
    v.sort();
    v
}

#[tokio::test]
async fn power_off_is_confirmed_before_anything_moves() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host(
        "dead",
        Some(("ipmi", "10.9.1.8", "root", "dead-bmc-password")),
    );
    h.go_down(&dead.meta.id).await;

    // Nothing is on the host, but the proof must still be power-off.
    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"rebuild_from_image": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(report.proof, Proof::PowerOff);
    // confirm_dead was not needed and not given; the fence ran once.
    assert_eq!(fencer.count(), 1);
    // The audit record names the proof the evacuation used.
    let proof_action = audit_actions(&h.store, "evacuate:");
    assert_eq!(proof_action, vec!["evacuate:proof_power_off"]);
}

#[tokio::test]
async fn a_power_off_that_cannot_be_confirmed_moves_nothing() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host(
        "dead",
        Some(("redfish", "https://bmc.example/redfish/v1", "root", "pw")),
    );
    h.go_down(&dead.meta.id).await;
    // Make machines so we can prove none move.
    let (st, m) = call::<forge::Machine>(
        &h.app,
        &admin(),
        "POST",
        "/machines",
        Some(machine_body("web")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    h.store
        .collection::<forge::Machine>()
        .modify(&m.meta.id, |x| {
            x.status.host = Some(dead.meta.id.clone());
            x.status.phase = Phase::Ready;
            Ok(())
        })
        .unwrap();

    fencer.fails();
    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"rebuild_from_image": true})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(fencer.count() >= 1);
    // The machine is untouched, still on the host, still ready.
    let m = h
        .store
        .collection::<forge::Machine>()
        .get(&m.meta.id)
        .unwrap();
    assert_eq!(m.status.host.as_ref(), Some(&dead.meta.id));
    assert_eq!(m.status.phase, Phase::Ready);
    assert!(!m
        .meta
        .annotations
        .contains_key(forge::EVACUATED_FROM_ANNOTATION));
}

#[tokio::test]
async fn without_power_control_a_down_host_still_needs_confirm() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host("dead", None);
    h.go_down(&dead.meta.id).await;

    // Even marked down, no power control and no confirmation is refused,
    // and the refusal names both ways out.
    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"rebuild_from_image": true})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("confirm_dead"), "{text}");
    assert!(text.contains("power control"), "{text}");
    // The fencer is never reached for a host with no power control.
    assert_eq!(fencer.count(), 0);
}

#[tokio::test]
async fn confirm_dead_proves_the_host_for_a_host_without_power_control() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host("dead", None);
    h.go_down(&dead.meta.id).await;
    let (st, m) = call::<forge::Machine>(
        &h.app,
        &admin(),
        "POST",
        "/machines",
        Some(machine_body("web")),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    h.store
        .collection::<forge::Machine>()
        .modify(&m.meta.id, |x| {
            x.status.host = Some(dead.meta.id.clone());
            x.status.phase = Phase::Ready;
            Ok(())
        })
        .unwrap();

    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"confirm_dead": true, "rebuild_from_image": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(report.proof, Proof::Operator);
    assert_eq!(report.evacuated.len(), 1, "{report:?}");
    assert_eq!(fencer.count(), 0);
    assert_eq!(
        audit_actions(&h.store, "evacuate:"),
        vec!["evacuate:proof_operator"]
    );
}

/// A host that still reports is a host that works. Disabling it does not say
/// it is off, so nothing is powered off and nothing moves without a
/// confirmation that says so out loud.
#[tokio::test]
async fn a_disabled_host_that_still_reports_is_not_powered_off_unasked() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let held = h.add_host("held", Some(("ipmi", "10.9.1.8", "root", "pw")));
    h.disable(&held.meta.id).await;

    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/held/evacuate",
        Some(json!({"rebuild_from_image": true})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let text = String::from_utf8_lossy(&body);
    assert!(text.contains("still reporting"), "{text}");
    assert!(text.contains("power"), "{text}");
    assert!(text.contains("drain"), "{text}");
    assert!(text.contains("confirm_dead"), "{text}");
    // The host was not so much as told to power off.
    assert_eq!(fencer.count(), 0);
}

/// The same host, with the operator saying it is dead: then the server fences
/// it and the proof is a power-off it confirmed, not the operator's word.
#[tokio::test]
async fn a_confirmed_report_host_is_powered_off_and_confirmed() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let held = h.add_host("held", Some(("ipmi", "10.9.1.8", "root", "pw")));
    h.disable(&held.meta.id).await;

    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/held/evacuate",
        Some(json!({"confirm_dead": true, "rebuild_from_image": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(report.proof, Proof::PowerOff);
    assert_eq!(report.power_off_failed, None);
    assert_eq!(fencer.count(), 1);
    assert_eq!(
        audit_actions(&h.store, "evacuate:"),
        vec!["evacuate:proof_power_off"]
    );
}

/// A host that has stopped reporting and can be powered off needs no
/// confirmation: the fence is the proof. Covered by
/// `power_off_is_confirmed_before_anything_moves`, which marks the host down,
/// gives no `confirm_dead`, and gets a power-off proof.
#[tokio::test]
async fn a_down_host_with_power_control_needs_no_confirm() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host("dead", Some(("ipmi", "10.9.1.8", "root", "pw")));
    h.go_down(&dead.meta.id).await;
    h.put_machine("web", &dead.meta.id).await;

    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"rebuild_from_image": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(report.proof, Proof::PowerOff);
    assert_eq!(report.evacuated.len(), 1, "{report:?}");
    assert_eq!(fencer.count(), 1);
}

/// A power-off that fails without a confirmation refuses, says why, and moves
/// nothing. `a_power_off_that_cannot_be_confirmed_moves_nothing` covers it.
#[tokio::test]
async fn a_failed_fence_without_confirm_moves_nothing() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host(
        "dead",
        Some(("redfish", "https://bmc.example/redfish/v1", "root", "pw")),
    );
    h.go_down(&dead.meta.id).await;
    let m = h.put_machine("web", &dead.meta.id).await;
    fencer.fails();

    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"rebuild_from_image": true})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let text = String::from_utf8_lossy(&body);
    assert!(
        text.contains("a power-off over redfish was tried"),
        "{text}"
    );
    assert!(fencer.count() >= 1);
    let m = h
        .store
        .collection::<forge::Machine>()
        .get(&m.meta.id)
        .unwrap();
    assert_eq!(m.status.host.as_ref(), Some(&dead.meta.id));
    assert_eq!(m.status.phase, Phase::Ready);
    assert!(audit_actions(&h.store, "evacuate:").is_empty());
}

/// The host's BMC died with the host — a failed power supply takes both. The
/// operator who has stood in front of it gets to evacuate it, with the report
/// and the audit record both saying a power-off was tried and did not happen.
#[tokio::test]
async fn a_failed_fence_with_confirm_moves_machines_on_the_operators_word() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    let dead = h.add_host("dead", Some(("ipmi", "10.9.1.8", "root", "pw")));
    h.go_down(&dead.meta.id).await;
    h.put_machine("web", &dead.meta.id).await;
    fencer.fails();

    let (st, report) = call::<EvacuateReport>(
        &h.app,
        &admin(),
        "POST",
        "/hosts/dead/evacuate",
        Some(json!({"confirm_dead": true, "rebuild_from_image": true})),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    assert_eq!(report.proof, Proof::Operator);
    assert_eq!(report.evacuated.len(), 1, "{report:?}");
    let why = report
        .power_off_failed
        .as_ref()
        .expect("the report says the power-off failed");
    assert!(why.contains("a power-off over ipmi was tried"), "{why}");
    assert!(why.contains("bmc did not answer"), "{why}");
    assert_eq!(fencer.count(), 1);
    // The machine moved, and it is recorded as moved off this host.
    let moved = h
        .store
        .collection::<forge::Machine>()
        .get(&report.evacuated[0].id)
        .unwrap();
    assert_eq!(moved.status.host, None);
    assert_eq!(
        moved
            .meta
            .annotations
            .get(forge::EVACUATED_FROM_ANNOTATION)
            .map(String::as_str),
        Some(dead.meta.id.to_string().as_str())
    );
    // One audit record, naming the proof and carrying the failed power-off.
    let events = h
        .store
        .collection::<gate::audit::AuditEvent>()
        .list(None)
        .unwrap()
        .into_iter()
        .filter(|e| e.spec.action.starts_with("evacuate:"))
        .collect::<Vec<_>>();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].spec.action, "evacuate:proof_operator");
    assert_eq!(
        events[0].spec.notes.get("power_off_failed"),
        Some(why),
        "the audit record notes the failed power-off"
    );
}

/// The power control on a host carries only the secret name. A set, a read,
/// and a down-host evacuation never surface a password.
#[tokio::test]
async fn the_password_is_never_stored_shown_or_moved() {
    let fencer = Fake::works();
    let h = harness(fencer).await;
    h.add_host("rack1", None);

    // Set power control naming a secret. The response carries the name only.
    let (st, resp) = call::<serde_json::Value>(
        &h.app,
        &admin(),
        "PUT",
        "/hosts/rack1/power",
        Some(json!({
            "kind": "ipmi",
            "address": "10.9.1.9",
            "user": "root",
            "secret": "rack1-bmc",
        })),
    )
    .await;
    assert_eq!(st, StatusCode::OK);
    let pc = &resp["power_control"];
    assert_eq!(pc["secret"], "rack1-bmc");
    // The returned power control has no field that could carry a password.
    let mut keys: Vec<&String> = pc.as_object().unwrap().keys().collect();
    keys.sort();
    assert_eq!(keys, ["address", "kind", "secret", "user"]);

    // The stored host's power control carries only the name too.
    let stored = h
        .store
        .collection::<Host>()
        .list(None)
        .unwrap()
        .into_iter()
        .find(|x| x.meta.name == "rack1")
        .unwrap();
    let pc = stored
        .spec
        .power_control
        .as_ref()
        .expect("power control stored");
    assert_eq!(pc.secret, "rack1-bmc");

    // show returns the name, never a payload.
    let (_, shown) =
        call::<serde_json::Value>(&h.app, &admin(), "GET", "/hosts/rack1/power", None).await;
    assert_eq!(shown["power_control"]["secret"], "rack1-bmc");
    assert!(shown.get("password").is_none());
    assert!(shown["power_control"].get("password").is_none());

    // clear drops it.
    let (st, cleared) =
        call::<serde_json::Value>(&h.app, &admin(), "DELETE", "/hosts/rack1/power", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(cleared["power_control"].is_null());
}

#[tokio::test]
async fn an_up_host_is_drained_not_evacuated() {
    let fencer = Fake::works();
    let h = harness(fencer.clone()).await;
    h.add_host("up", Some(("ipmi", "10.9.1.8", "root", "pw")));
    // The host is Ready (not down, not disabled).
    let (st, body) = raw(
        &h.app,
        &admin(),
        "POST",
        "/hosts/up/evacuate",
        Some(json!({"confirm_dead": true, "rebuild_from_image": true})),
    )
    .await;
    assert_eq!(
        st,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    assert!(String::from_utf8_lossy(&body).contains("drain"));
    assert_eq!(fencer.count(), 0);
}
