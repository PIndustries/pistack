// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End to end: create a zone and an A record through the HTTP router, query
//! the authoritative server over UDP, watch the serial move on update.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use beacon::*;
use hickory_server::net::client::{Client as DnsClient, ClientHandle};
use hickory_server::net::runtime::TokioRuntimeProvider;
use hickory_server::net::udp::UdpClientStream;
use hickory_server::proto::op::ResponseCode;
use hickory_server::proto::rr::{DNSClass, Name, RData, RecordType as Rt};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tower::ServiceExt;

struct Harness {
    module: Beacon,
    ctx: ModuleContext,
    router: axum::Router,
    dns: SocketAddr,
}

async fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let module = Beacon::with_listen("127.0.0.1:0".parse().unwrap());
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    module.start(ctx.clone()).await.unwrap();
    let dns = module.bound_addr().unwrap();
    let router = module.router(ctx.clone());
    Harness {
        module,
        ctx,
        router,
        dns,
    }
}

impl Harness {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        let body = match body {
            Some(v) => {
                req = req.header(header::CONTENT_TYPE, "application/json");
                Body::from(v.to_string())
            }
            None => Body::empty(),
        };
        let mut req = req.body(body).unwrap();
        req.extensions_mut().insert(admin_caller());
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let v = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or(Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status, v)
    }

    async fn query(&self, name: &str, rt: Rt) -> hickory_server::proto::op::DnsResponse {
        let conn = UdpClientStream::builder(self.dns, TokioRuntimeProvider::default()).build();
        let (mut client, bg) = DnsClient::<TokioRuntimeProvider>::from_sender(conn);
        tokio::spawn(bg);
        client
            .query(Name::from_str(name).unwrap(), DNSClass::IN, rt)
            .await
            .unwrap()
    }

    /// Poll until the zone's serial satisfies `pred`.
    async fn wait_serial(&self, zone: &str, pred: impl Fn(u32) -> bool) -> u32 {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let (_, z) = self.call("GET", &format!("/zones/{zone}"), None).await;
            let serial = z["status"]["serial"].as_u64().unwrap_or(0) as u32;
            if pred(serial) {
                return serial;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "serial did not move: {z}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

#[tokio::test]
async fn zone_and_record_over_http_then_udp() {
    let h = harness().await;
    assert_eq!(h.module.info().prefix, "dns");

    let (st, zone) = h
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "example",
                "spec": { "name": "Example.Cloud", "email": "ops@example.cloud",
                          "nameservers": ["ns1.example.cloud."] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{zone}");
    assert_eq!(zone["spec"]["name"], "example.cloud.");
    let zone_id = zone["id"].as_str().unwrap().to_owned();
    let first = h.wait_serial(&zone_id, |s| s >= 1).await;

    let (st, rec) = h
        .call(
            "POST",
            "/recordsets",
            Some(json!({
                "name": "web-a",
                "spec": { "zone": zone_id, "name": "web", "type": "A",
                          "ttl": 60, "records": ["10.0.0.4", "10.0.0.5"] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{rec}");
    assert_eq!(rec["spec"]["name"], "web.example.cloud.");
    let rec_id = rec["id"].as_str().unwrap().to_owned();
    let second = h.wait_serial(&zone_id, |s| s > first).await;

    // Over the wire.
    let resp = h.query("web.example.cloud.", Rt::A).await;
    assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
    assert!(resp.metadata.authoritative);
    let mut ips: Vec<String> = resp
        .answers
        .iter()
        .filter_map(|r| match &r.data {
            RData::A(a) => Some(a.0.to_string()),
            _ => None,
        })
        .collect();
    ips.sort();
    assert_eq!(ips, vec!["10.0.0.4", "10.0.0.5"]);
    assert_eq!(resp.answers[0].ttl, 60);

    // Case-insensitive lookup and synthesised SOA with the current serial.
    let soa = h.query("EXAMPLE.cloud.", Rt::SOA).await;
    match &soa.answers[0].data {
        RData::SOA(s) => {
            assert_eq!(s.serial, second);
            assert_eq!(s.mname.to_ascii(), "ns1.example.cloud.");
            assert_eq!(s.rname.to_ascii(), "ops.example.cloud.");
        }
        other => panic!("expected SOA, got {other:?}"),
    }
    let ns = h.query("example.cloud.", Rt::NS).await;
    assert_eq!(ns.answers.len(), 1);

    // NXDOMAIN carries the SOA; a foreign name is refused.
    let nx = h.query("nope.example.cloud.", Rt::A).await;
    assert_eq!(nx.metadata.response_code, ResponseCode::NXDomain);
    assert_eq!(nx.authorities[0].record_type(), Rt::SOA);
    let foreign = h.query("example.org.", Rt::A).await;
    assert_eq!(foreign.metadata.response_code, ResponseCode::Refused);

    // Update the record: serial increments, server answers the new data.
    let (st, upd) = h
        .call(
            "PUT",
            &format!("/recordsets/{rec_id}"),
            Some(json!({
                "spec": { "zone": zone_id, "name": "web", "type": "A", "records": ["10.0.0.9"] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{upd}");
    let third = h.wait_serial(&zone_id, |s| s > second).await;
    assert_eq!(third, second + 1);
    let resp = h.query("web.example.cloud.", Rt::A).await;
    assert_eq!(resp.answers.len(), 1);
    assert_eq!(resp.answers[0].ttl, 3600, "ttl falls back to the zone's");

    // Stale If-Match conflicts.
    let (st, _) = h
        .call(
            "PUT",
            &format!("/recordsets/{rec_id}"),
            Some(json!({ "spec": { "zone": zone_id, "name": "web", "type": "A", "records": ["10.0.0.1"] } })),
        )
        .await;
    assert_eq!(st, StatusCode::OK);
    let mut stale = Request::builder()
        .method("PUT")
        .uri(format!("/recordsets/{rec_id}"))
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::IF_MATCH, "1")
        .body(Body::from(
            json!({ "spec": { "zone": zone_id, "name": "web", "type": "A", "records": ["10.0.0.2"] } })
                .to_string(),
        ))
        .unwrap();
    stale.extensions_mut().insert(admin_caller());
    let res = h.router.clone().oneshot(stale).await.unwrap();
    assert_eq!(res.status(), StatusCode::CONFLICT);

    // Export and list.
    let (st, text) = h
        .call("GET", &format!("/zones/{zone_id}/export"), None)
        .await;
    assert_eq!(st, StatusCode::OK);
    let text = text.as_str().unwrap();
    assert!(text.contains("$ORIGIN example.cloud."));
    assert!(text.contains("web.example.cloud. 3600 IN A 10.0.0.1"));
    let (_, list) = h
        .call("GET", &format!("/recordsets?zone={zone_id}"), None)
        .await;
    assert_eq!(list["total"], 1);
    // Zone resolves by apex too.
    let (st, _) = h.call("GET", "/zones/example.cloud.", None).await;
    assert_eq!(st, StatusCode::OK);

    // Validation errors are 400; CNAME next to A is 409.
    let (st, _) = h
        .call(
            "POST",
            "/recordsets",
            Some(json!({ "name": "bad", "spec": { "zone": zone_id, "name": "x", "type": "A", "records": ["nope"] } })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, _) = h
        .call(
            "POST",
            "/recordsets",
            Some(json!({ "name": "web-c", "spec": { "zone": zone_id, "name": "web", "type": "CNAME", "records": ["a.example.cloud."] } })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);

    // Delete cascades and the zone is gone from the wire.
    let (st, _) = h.call("DELETE", &format!("/zones/{zone_id}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = h.call("GET", &format!("/recordsets/{rec_id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let r = h.query("web.example.cloud.", Rt::A).await;
        if r.metadata.response_code == ResponseCode::Refused {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "cache did not invalidate"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    h.ctx.shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

#[tokio::test]
async fn auto_records_follow_machines() {
    let h = harness().await;
    let project = Id::new(kind::PROJECT);
    let (st, zone) = h
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "auto", "project": project.to_string(),
                "labels": { "pistack.io/auto": "true" },
                "spec": { "name": "auto.cloud." }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{zone}");

    // A machine appears the way forge would store it.
    let store = h.ctx.store::<Store>();
    let col = store.collection::<RawMachine>();
    let mut m = pistack_types::Meta::new(kind::MACHINE, "db", Some(project.clone()));
    m.labels.insert("x".into(), "y".into());
    col.create(RawMachine {
        meta: m.clone(),
        spec: json!({}),
        status: json!({ "addresses": ["10.1.0.7"] }),
    })
    .unwrap();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let r = h.query("db.auto.cloud.", Rt::A).await;
        if r.metadata.response_code == ResponseCode::NoError && !r.answers.is_empty() {
            assert!(matches!(&r.answers[0].data, RData::A(a) if a.0.to_string() == "10.1.0.7"));
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "auto record not served"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    col.delete(&m.id).unwrap();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let r = h.query("db.auto.cloud.", Rt::A).await;
        if r.metadata.response_code == ResponseCode::NXDomain {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "auto record not removed"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn secondary_axfr_from_local_primary() {
    let primary = harness().await;

    let (st, zone) = primary
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "primary",
                "spec": {
                    "name": "xfer.cloud.",
                    "email": "ops@xfer.cloud",
                    "nameservers": ["ns1.xfer.cloud."]
                }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{zone}");
    let zone_id = zone["id"].as_str().unwrap().to_owned();
    primary.wait_serial(&zone_id, |s| s >= 1).await;
    let (st, _) = primary
        .call(
            "POST",
            "/recordsets",
            Some(json!({
                "name": "web-a",
                "spec": {
                    "zone": zone_id,
                    "name": "web",
                    "type": "A",
                    "records": ["10.9.8.7"]
                }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    primary.wait_serial(&zone_id, |s| s >= 2).await;

    // Second beacon with its own store: secondary-only zone pulled over AXFR.
    let store2 = Store::in_memory().unwrap();
    let module2 = Beacon::with_listen("127.0.0.1:0".parse().unwrap());
    let ctx2 = ModuleContext {
        store: Arc::new(store2.clone()),
        events: store2.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test2".into(),
        node_name: "test2".into(),
        leadership: pistack_types::Leadership::always(),
    };
    module2.start(ctx2.clone()).await.unwrap();
    let dns2 = module2.bound_addr().unwrap();
    let router2 = module2.router(ctx2.clone());

    let mut req = Request::builder()
        .method("POST")
        .uri("/zones")
        .header(header::CONTENT_TYPE, "application/json")
        .body(Body::from(
            json!({
                "name": "secondary",
                "spec": {
                    "name": "xfer.cloud.",
                    "kind": {
                        "type": "secondary",
                        "primaries": [primary.dns.to_string()]
                    },
                    "email": "ops@xfer.cloud",
                    "nameservers": ["ns1.xfer.cloud."]
                }
            })
            .to_string(),
        ))
        .unwrap();
    req.extensions_mut().insert(admin_caller());
    let res = router2.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let sec: Value = serde_json::from_slice(&bytes).unwrap();
    let sec_id = sec["id"].as_str().unwrap().to_owned();

    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let mut req = Request::builder()
            .method("GET")
            .uri(format!("/zones/{sec_id}"))
            .body(Body::empty())
            .unwrap();
        req.extensions_mut().insert(admin_caller());
        let res = router2.clone().oneshot(req).await.unwrap();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let z: Value = serde_json::from_slice(&bytes).unwrap();
        if z["status"]["phase"] == "ready" {
            assert!(z["status"]["serial"].as_u64().unwrap() >= 2, "{z}");
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "secondary not ready: {z}"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    let conn = UdpClientStream::builder(dns2, TokioRuntimeProvider::default()).build();
    let (mut client, bg) = DnsClient::<TokioRuntimeProvider>::from_sender(conn);
    tokio::spawn(bg);
    let resp = client
        .query(
            Name::from_str("web.xfer.cloud.").unwrap(),
            DNSClass::IN,
            Rt::A,
        )
        .await
        .unwrap();
    assert_eq!(resp.metadata.response_code, ResponseCode::NoError);
    let ip = resp.answers.iter().find_map(|r| match &r.data {
        RData::A(a) => Some(a.0.to_string()),
        _ => None,
    });
    assert_eq!(ip.as_deref(), Some("10.9.8.7"));

    ctx2.shutdown.cancel();
    primary.ctx.shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Stand-in for forge's `Machine`, sharing its wire shape and store key.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct RawMachine {
    #[serde(flatten)]
    meta: pistack_types::Meta,
    spec: Value,
    status: Value,
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
    let store = Store::in_memory().unwrap();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let app = Beacon::default().router(ctx);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/zones")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
        .await
        .unwrap();
    let body: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(body["error"]["code"], "unauthorized");
    assert_eq!(body["error"]["message"], "no credentials");
}

impl pistack_types::Kind for RawMachine {
    const KIND: &'static str = kind::MACHINE;
    fn meta(&self) -> &pistack_types::Meta {
        &self.meta
    }
    fn meta_mut(&mut self) -> &mut pistack_types::Meta {
        &mut self.meta
    }
}
