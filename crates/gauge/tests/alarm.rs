// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Post a sample, create an alarm, post a crossing sample: the alarm fires.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use gauge::Gauge;
use pistack_store::Store;
use pistack_types::module::CancellationToken;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use std::sync::Arc;
use tower::ServiceExt;

struct Harness {
    router: axum::Router,
}

fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let gauge = Gauge;
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    assert_eq!(gauge.info().name, "gauge");
    assert_eq!(gauge.info().prefix, "gauge");
    Harness {
        router: gauge.router(ctx),
    }
}

impl Harness {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, String) {
        self.call_as(admin_caller(), method, path, body).await
    }

    async fn call_as(
        &self,
        caller: Caller,
        method: &str,
        path: &str,
        body: Option<Value>,
    ) -> (StatusCode, String) {
        let mut req = Request::builder().method(method).uri(path);
        let body = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let mut req = req.body(body).unwrap();
        req.extensions_mut().insert(caller);
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }
}

fn json_body(status: StatusCode, raw: &str) -> Value {
    if raw.is_empty() {
        return Value::Null;
    }
    serde_json::from_str(raw).unwrap_or_else(|e| panic!("status {status} not json ({e}): {raw}"))
}

#[tokio::test]
async fn crossing_sample_fires_alarm() {
    let h = harness();
    // Count from whatever the ring already holds, so samples left by an
    // earlier run or another test cannot change the result.
    let (st, raw) = h.call("GET", "/samples?metric=cpu.usage", None).await;
    let before = json_body(st, &raw).as_array().map_or(0, Vec::len);

    let (st, raw) = h
        .call(
            "POST",
            "/samples",
            Some(json!({"metric": "cpu.usage", "value": 10.0})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");

    let (st, raw) = h
        .call(
            "POST",
            "/alarms",
            Some(json!({
                "name": "cpu-high",
                "spec": {
                    "name": "cpu-high",
                    "metric": "cpu.usage",
                    "comparison": "gt",
                    "threshold": 50.0,
                    "for_seconds": 0
                }
            })),
        )
        .await;
    let alarm = json_body(st, &raw);
    assert_eq!(st, StatusCode::CREATED, "{alarm}");
    let id = alarm["id"].as_str().unwrap();
    assert!(id.starts_with("alarm-"), "{id}");
    assert_eq!(alarm["status"]["phase"], "Ok");
    assert_eq!(alarm["status"]["last_value"], 10.0);

    let (st, raw) = h
        .call(
            "POST",
            "/samples",
            Some(json!({"metric": "cpu.usage", "value": 80.0})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{raw}");

    let (st, raw) = h.call("GET", &format!("/alarms/{id}"), None).await;
    let alarm = json_body(st, &raw);
    assert_eq!(st, StatusCode::OK, "{alarm}");
    assert_eq!(alarm["status"]["phase"], "Firing");
    assert_eq!(alarm["status"]["last_value"], 80.0);

    let (st, text) = h.call("GET", "/metrics", None).await;
    assert_eq!(st, StatusCode::OK, "{text}");
    assert!(
        text.lines()
            .any(|l| l == "cpu_usage 80" || l == "cpu_usage 80.0"),
        "{text}"
    );

    let (st, raw) = h.call("GET", "/samples?metric=cpu.usage", None).await;
    let samples = json_body(st, &raw);
    assert_eq!(st, StatusCode::OK, "{samples}");
    assert_eq!(samples.as_array().unwrap().len(), before + 2);
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

#[tokio::test]
async fn missing_caller_is_unauthorized() {
    let h = harness();
    let res = h
        .router
        .clone()
        .oneshot(
            Request::builder()
                .uri("/alarms")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let status = res.status();
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let raw = String::from_utf8(
        axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
    .unwrap();
    let body = json_body(status, &raw);
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}

fn member_of(project: &Id) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "member".into(),
        project: Some(project.clone()),
        roles: vec!["member".into()],
        system: false,
    }
}

#[tokio::test]
async fn pistack_series_are_for_the_admin_and_each_project_sees_its_own() {
    let h = harness();
    let mine = Id::new(kind::PROJECT);
    let theirs = Id::new(kind::PROJECT);
    let memory = |machine: &str, project: &Id| {
        gauge::series(
            gauge::catalog::MACHINE_MEMORY,
            &[("machine", machine), ("project", &project.to_string())],
        )
    };
    gauge::Gauge
        .client(Store::in_memory().unwrap())
        .record_all(vec![
            (memory("scope-mine", &mine), 1.0),
            (memory("scope-theirs", &theirs), 2.0),
            (
                gauge::series(gauge::catalog::HOST_LOAD1, &[("host", "scope-h1")]),
                3.0,
            ),
        ]);
    let member = member_of(&mine);

    // The exposition has every project and host in it: admin only.
    let (st, text) = h.call("GET", "/metrics", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(
        text.contains("scope-theirs") && text.contains("scope-h1"),
        "{text}"
    );
    let (st, raw) = h.call_as(member.clone(), "GET", "/metrics", None).await;
    assert_eq!(st, StatusCode::FORBIDDEN, "{raw}");
    assert!(!raw.contains("scope-"), "{raw}");

    // The samples a member lists leave out hosts and other projects.
    let (st, raw) = h.call_as(member.clone(), "GET", "/samples", None).await;
    assert_eq!(st, StatusCode::OK);
    assert!(raw.contains("scope-mine"), "{raw}");
    assert!(
        !raw.contains("scope-theirs") && !raw.contains("scope-h1"),
        "{raw}"
    );
    let (_, raw) = h.call("GET", "/samples", None).await;
    assert!(
        raw.contains("scope-theirs") && raw.contains("scope-h1"),
        "{raw}"
    );

    // Nobody posts under PiStack's own names, the admin included.
    for caller in [member, admin_caller()] {
        let body = json!({"metric": "pistack_host_load1{host=\"scope-h1\"}", "value": 99.0});
        let (st, raw) = h.call_as(caller, "POST", "/samples", Some(body)).await;
        assert_eq!(st, StatusCode::BAD_REQUEST, "{raw}");
    }
}
