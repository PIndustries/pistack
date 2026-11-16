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
    harness_with(Store::in_memory().unwrap()).await
}

/// A beacon on a store of the caller's choosing, so a test can restart one
/// over data it already holds.
async fn harness_with(store: Store) -> Harness {
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

    /// The zone's current serial, `0` when the store has none yet.
    async fn zone_serial(&self, zone: &str) -> u32 {
        let (_, z) = self.call("GET", &format!("/zones/{zone}"), None).await;
        z["status"]["serial"].as_u64().unwrap_or(0) as u32
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

/// A NOTIFY as a primary sends it: the same wire format beacon's notifier uses,
/// over a socket of the test's own, so the refusal path can be seen from the
/// outside rather than inferred from a status field.
///
/// `from` is a host to speak as; the port the packet leaves from is the OS's, as
/// it is for a real notifier, since a peer's own port is not ours to bind.
/// Beacon matches the address a zone lists on host alone, so that host is all
/// acceptance sees. `serial` is what the sender claims; acceptance does not
/// consult it.
async fn notify_as(from: SocketAddr, to: SocketAddr, apex: &str, serial: u32) -> ResponseCode {
    let bytes = beacon::notify::notify_message(apex, serial).unwrap();
    let from = SocketAddr::new(from.ip(), 0);
    let sock = tokio::net::UdpSocket::bind(from).await.unwrap();
    sock.connect(to).await.unwrap();
    sock.send(&bytes).await.unwrap();
    let mut buf = [0u8; 2048];
    let n = tokio::time::timeout(Duration::from_secs(2), sock.recv(&mut buf))
        .await
        .expect("no answer to a NOTIFY")
        .unwrap();
    hickory_server::proto::op::Message::from_vec(&buf[..n])
        .unwrap()
        .metadata
        .response_code
}

/// The primary's change reaches the secondary inside a second. The refresh
/// timer runs at thirty, so a change that lands this fast arrived on the
/// NOTIFY.
#[tokio::test]
async fn a_serial_change_reaches_the_secondary_by_notify() {
    let primary = harness().await;
    let secondary = harness().await;
    let (st, zone) = primary
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "n1",
                "spec": { "name": "n.cloud.", "email": "ops@n.cloud",
                          "nameservers": ["ns1.n.cloud."] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{zone}");
    let zone_id = zone["id"].as_str().unwrap().to_owned();
    let (st, _) = primary
        .call(
            "POST",
            "/recordsets",
            Some(json!({
                "name": "web-a",
                "spec": { "zone": zone_id, "name": "web", "type": "A",
                          "records": ["10.20.30.40"] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let serial = primary.wait_serial(&zone_id, |s| s >= 1).await;

    // The secondary lists the primary and, being a secondary, starts empty.
    let (st, _) = secondary
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "n1",
                "spec": { "name": "n.cloud.", "email": "ops@n.cloud",
                          "nameservers": ["ns1.n.cloud."],
                          "kind": { "type": "secondary",
                                    "primaries": [primary.dns.to_string()] } }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(
        secondary
            .query("web.n.cloud.", Rt::A)
            .await
            .metadata
            .response_code,
        ResponseCode::Refused,
        "nothing of the primary's should be served yet"
    );

    // What a primary does when its serial moves: NOTIFY each address that lists
    // it as a primary.
    //
    // The refresh timer runs at thirty seconds, so a zone that fills this fast
    // filled because of the NOTIFY.
    let started = tokio::time::Instant::now();
    let peer = secondary.dns.to_string();
    beacon::notify::notify_peers(&[peer], "n.cloud.", serial)
        .await
        .expect("the NOTIFY did not reach the secondary");

    loop {
        let resp = secondary.query("web.n.cloud.", Rt::A).await;
        if resp.metadata.response_code == ResponseCode::NoError {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the zone took {:?} to fill — the refresh timer is 30s, so this is \
             not a NOTIFY",
            started.elapsed()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let serial_now = secondary.zone_serial("n1").await;
    assert!(
        serial_now >= serial,
        "the secondary pulled a stale copy: {serial_now} < {serial}"
    );

    secondary.ctx.shutdown.cancel();
    primary.ctx.shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// A NOTIFY is believed by host. One from a host no secondary zone lists is
/// refused and pulls nothing; the primary's own host is answered, and the pull
/// it asks for happens.
#[tokio::test]
async fn a_notify_is_accepted_only_from_the_listed_primary() {
    let source = harness().await;
    let (st, zone) = source
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "s1",
                "spec": { "name": "q.cloud.", "email": "ops@q.cloud",
                          "nameservers": ["ns1.q.cloud."] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{zone}");
    let zone_id = zone["id"].as_str().unwrap().to_owned();
    let (st, _) = source
        .call(
            "POST",
            "/recordsets",
            Some(json!({
                "name": "web-a",
                "spec": { "zone": zone_id, "name": "web", "type": "A",
                          "records": ["10.31.31.31"] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let serial = source.wait_serial(&zone_id, |s| s >= 1).await;

    // The node that will be notified. It is given the zone with a primary
    // nothing answers at, so the periodic transfer fails and cannot fill the
    // zone behind the test's back. The primary's real address goes in later,
    // just before its NOTIFY, and the periodic pass runs at thirty seconds — so
    // anything that fills this zone in the meantime arrived on the NOTIFY.
    let node = harness().await;
    let (st, _) = node
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "s1",
                "spec": { "name": "q.cloud.", "email": "ops@q.cloud",
                          "nameservers": ["ns1.q.cloud."],
                          "kind": { "type": "secondary",
                                    "primaries": ["192.0.2.1"] } }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);

    // A second beacon on loopback, holding nothing of this zone. A secondary
    // matches a NOTIFY against the primaries of its own zones, so a sender
    // trusted nowhere must be refused — and refused, not pulled.
    let neighbour = harness().await;
    let (st, _) = neighbour
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "s1-foreign",
                "spec": { "name": "q.cloud.", "email": "ops@q.cloud",
                          "nameservers": ["ns1.q.cloud."],
                          "kind": { "type": "primary" } }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(
        notify_as(neighbour.dns, node.dns, "q.cloud.", serial + 500).await,
        ResponseCode::Refused,
        "a sender listed by no secondary zone was accepted"
    );
    // A NOTIFY for a zone this node does not hold is refused too, even from the
    // primary it does trust.
    assert_eq!(
        notify_as(source.dns, node.dns, "elsewhere.cloud.", 1).await,
        ResponseCode::Refused,
        "a notify for a zone nobody holds was accepted"
    );

    // The refusal is not cosmetic: the zone is untouched, still waiting on the
    // transfer a NOTIFY would have started. (The notify for the zone nobody
    // holds cannot move anything by construction.)
    let (_, z) = node.call("GET", "/zones/s1", None).await;
    assert_eq!(
        z["status"]["phase"].as_str(),
        Some("pending"),
        "a refused notify moved the zone: {z}"
    );
    assert_eq!(
        node.query("web.q.cloud.", Rt::A)
            .await
            .metadata
            .response_code,
        ResponseCode::Refused,
        "a refused notify pulled the zone"
    );

    // Now the primary's address, as an operator would set it. Nothing pulls
    // yet: the refresh pass that would is still a long way off.
    let (st, upd) = node
        .call(
            "PUT",
            "/zones/s1",
            Some(json!({
                "spec": { "name": "q.cloud.", "email": "ops@q.cloud",
                          "nameservers": ["ns1.q.cloud."],
                          "kind": { "type": "secondary",
                                    "primaries": [source.dns.to_string()] } }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{upd}");
    let (_, z) = node.call("GET", "/zones/s1", None).await;
    assert_eq!(
        z["status"]["phase"].as_str(),
        Some("pending"),
        "the zone transferred before anything asked it to: {z}"
    );

    // And the primary, whose address the zone now lists, asks for the pull.
    assert_eq!(
        notify_as(source.dns, node.dns, "q.cloud.", serial).await,
        ResponseCode::NoError,
        "a listed primary was refused"
    );
    let started = tokio::time::Instant::now();
    loop {
        let resp = node.query("web.q.cloud.", Rt::A).await;
        let ip = resp.answers.iter().find_map(|r| match &r.data {
            RData::A(a) => Some(a.0.to_string()),
            _ => None,
        });
        if ip.as_deref() == Some("10.31.31.31") {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "an accepted notify did not pull the zone"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    neighbour.ctx.shutdown.cancel();
    node.ctx.shutdown.cancel();
    source.ctx.shutdown.cancel();
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// What an `also_notify` listener saw, and when each send arrived.
#[derive(Clone)]
struct Seen {
    hits: Arc<parking_lot::Mutex<Vec<std::time::Instant>>>,
}

/// A stand-in for an outside secondary: a plain UDP listener that answers a
/// NOTIFY and records every send. With `silent` it drops the first send and
/// answers from the second on, which is how a lost answer looks to the sender.
async fn listener(silent: bool) -> (SocketAddr, Seen) {
    let sock = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    let addr = sock.local_addr().unwrap();
    let seen = Seen {
        hits: Arc::new(parking_lot::Mutex::new(Vec::new())),
    };
    let hits = seen.hits.clone();
    tokio::spawn(async move {
        let mut buf = [0u8; 4096];
        let mut got = 0usize;
        loop {
            let Ok((n, from)) = sock.recv_from(&mut buf).await else {
                break;
            };
            let Ok(msg) = hickory_server::proto::op::Message::from_vec(&buf[..n]) else {
                continue;
            };
            if msg.metadata.op_code != hickory_server::proto::op::OpCode::Notify {
                continue;
            }
            hits.lock().push(std::time::Instant::now());
            got += 1;
            // A silent listener ignores the first send; the answer it eventually
            // sends is only an ack, never the point.
            if silent && got == 1 {
                continue;
            }
            let mut answer = msg.clone();
            answer.metadata.message_type = hickory_server::proto::op::MessageType::Response;
            answer.metadata.response_code = hickory_server::proto::op::ResponseCode::NoError;
            let _ = sock.send_to(&answer.to_vec().unwrap(), from).await;
        }
    });
    (addr, seen)
}

/// A primary's change reaches a server listed only in `also_notify` — an outside
/// DNS that holds nothing in the store — inside a second, and a send whose
/// answer is lost is tried again. The zone names the listener and nothing else,
/// so nothing but `also_notify` can put a packet there.
#[tokio::test]
async fn an_also_notify_listener_is_reached_and_retried() {
    // A listener that ignores the first send and answers the second: the lost
    // answer the primary has to retry through.
    let (peer, seen) = listener(true).await;

    let h = harness().await;
    let peer_str = peer.to_string();

    // A secondary refuses the field before it can be stored anywhere.
    let (st, _) = h
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "sec",
                "spec": { "name": "an.cloud.", "email": "ops@an.cloud",
                          "nameservers": ["ns1.an.cloud."],
                          "also_notify": [peer_str.clone()],
                          "kind": { "type": "secondary",
                                    "primaries": ["192.0.2.9"] } }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a secondary took also_notify");

    // Too many addresses, and an address that is not one, are refused too.
    let many: Vec<String> = (0..17).map(|i| format!("198.51.100.{}", i % 251)).collect();
    let (st, _) = h
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "many",
                "spec": { "name": "many.cloud.", "email": "ops@many.cloud",
                          "nameservers": ["ns1.many.cloud."],
                          "also_notify": many }
            })),
        )
        .await;
    assert_eq!(
        st,
        StatusCode::BAD_REQUEST,
        "seventeen addresses were taken"
    );
    let (st, _) = h
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "junk",
                "spec": { "name": "junk.cloud.", "email": "ops@junk.cloud",
                          "nameservers": ["ns1.junk.cloud."],
                          "also_notify": ["not-an-address"] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST, "a non-address was taken");

    // The primary, naming the listener and no in-cloud secondary.
    let (st, zone) = h
        .call(
            "POST",
            "/zones",
            Some(json!({
                "name": "an",
                "spec": { "name": "an.cloud.", "email": "ops@an.cloud",
                          "nameservers": ["ns1.an.cloud."],
                          "also_notify": [peer_str.clone()] }
            })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{zone}");
    let zone_id = zone["id"].as_str().unwrap().to_owned();
    let (_st, _) = h
        .call(
            "POST",
            "/recordsets",
            Some(json!({
                "name": "web-a",
                "spec": { "zone": zone_id, "name": "web", "type": "A",
                          "records": ["10.40.40.40"] }
            })),
        )
        .await;
    h.wait_serial(&zone_id, |s| s >= 1).await;

    // A change is on its way: fire exactly what a leader fires when the serial
    // moves — the real notify path over the zone's stored peers.
    let started = std::time::Instant::now();
    let client = h.module.client(&h.ctx);
    beacon::notify::notify_one(&client, "an.cloud.").await;

    // The listener saw it, and quickly: the refresh timer is thirty seconds, so
    // a send this soon is the NOTIFY, not a poll.
    loop {
        if !seen.hits.lock().is_empty() {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "the also_notify listener was never asked"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let first = *seen.hits.lock().first().unwrap();
    assert!(
        first.duration_since(started) < Duration::from_secs(1),
        "the listener was reached {:?} after the change",
        first.duration_since(started)
    );
    // The first answer was lost, so the same NOTIFY is tried again.
    loop {
        if seen.hits.lock().len() >= 2 {
            break;
        }
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "the lost answer was not retried"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    h.ctx.shutdown.cancel();
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

/// The reported bug, end to end and with no restart.
///
/// One zone, three servers. The leader takes every write; a follower copies
/// the same rows out of the log, straight into its backend, so nothing on this
/// node is ever written through a `Collection` and nothing used to be sent on
/// the event bus. The DNS handler kept the catalog it compiled the first time
/// a query asked it something: a zone the leader deleted was answered for good,
/// and a zone created after this server started was never answered at all.
///
/// The follower here answers from its own handler, and its store changes only
/// through `apply_entries`, with pages read out of a second, leading store, the
/// way the follow loop hands them over.
#[tokio::test]
async fn a_follower_answers_and_stops_answering_as_the_leader_deletes() {
    let leader_store = Store::in_memory().unwrap();
    leader_store.set_record_log(true);
    let leader = Client::new(leader_store.clone());

    let follower = Store::in_memory().unwrap();
    let handler =
        server::Handler::new(Client::new(follower.clone()), server::SecondaryCache::new());
    handler.watch(follower.events().subscribe(), Default::default());

    // First catch-up: a zone with nothing in it, so the handler has a catalog
    // compiled at all. Answered, and only the apex: nothing points at `web`.
    let zone = new_zone(&leader, "gone.cloud.");
    follow_once(&leader_store, &follower).await;
    assert_eq!(
        code(&handler, "gone.cloud.", Rt::SOA).await,
        ResponseCode::NoError,
        "a follower answers the zone it was handed"
    );
    assert_eq!(
        code(&handler, "web.gone.cloud.", Rt::A).await,
        ResponseCode::NXDomain
    );

    // A record set appears while the server is running.
    record(&leader, &zone.meta.id, "web", &["10.0.0.4"]);
    follow_once(&leader_store, &follower).await;
    wait_code(&handler, "web.gone.cloud.", Rt::A, ResponseCode::NoError).await;

    // The leader deletes the record set. It goes, and the zone stays.
    let rec = leader
        .recordsets_in_zone(&zone.meta.id)
        .unwrap()
        .into_iter()
        .find(|r| r.spec.name == "web.gone.cloud.")
        .unwrap();
    leader.recordsets().delete(&rec.meta.id).unwrap();
    follow_once(&leader_store, &follower).await;
    wait_code(&handler, "web.gone.cloud.", Rt::A, ResponseCode::NXDomain).await;
    assert_eq!(
        code(&handler, "gone.cloud.", Rt::SOA).await,
        ResponseCode::NoError
    );

    // The leader deletes the zone. The follower stops answering for the name
    // entirely, exactly as it answers for a name it never saw.
    leader.zones().delete(&zone.meta.id).unwrap();
    follow_once(&leader_store, &follower).await;
    wait_code(&handler, "gone.cloud.", Rt::SOA, ResponseCode::Refused).await;

    // And one made afterwards is answered: a follower is not left with whatever
    // its store held the first time anything asked it a question.
    let _later = new_zone(&leader, "later.cloud.");
    follow_once(&leader_store, &follower).await;
    wait_code(&handler, "later.cloud.", Rt::SOA, ResponseCode::NoError).await;
}

/// Hand the follower everything the leader has logged and not yet given it.
async fn follow_once(leader: &Store, follower: &Store) {
    let from = follower.commit_index().unwrap();
    let page = leader.read_log(from, 64).unwrap();
    follower.apply_entries(&page).unwrap();
    // The watcher runs beside the query path. Let it take the events.
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

fn new_zone(client: &Client, apex: &str) -> Zone {
    client
        .create_zone(
            pistack_types::Meta::new(kind::ZONE, apex.trim_end_matches('.'), None),
            ZoneSpec {
                name: apex.into(),
                ..ZoneSpec::default()
            },
        )
        .unwrap()
}

fn record(client: &Client, zone: &Id, name: &str, records: &[&str]) {
    client
        .upsert_record(
            zone,
            name,
            beacon::RecordType::A,
            None,
            records.iter().map(|r| r.to_string()).collect(),
            "test",
        )
        .unwrap();
}

/// What the handler answers, as a response code, with no socket in the way.
async fn code(handler: &server::Handler, name: &str, rt: Rt) -> ResponseCode {
    let name = Name::from_str(name).unwrap();
    match handler.resolve(&name, rt).unwrap() {
        Some(a) => a.code,
        // No zone of ours covers the name: the server refuses it.
        None => ResponseCode::Refused,
    }
}

async fn wait_code(handler: &server::Handler, name: &str, rt: Rt, want: ResponseCode) {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        if code(handler, name, rt).await == want {
            return;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "{name} never came to answer {want}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}
