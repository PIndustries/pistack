// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end tests over `Store::in_memory` and `MockProvider`.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext, Phase};
use relay::health::Checker;
use relay::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tower::ServiceExt;

struct Harness {
    ctx: ModuleContext,
    client: Client,
    provider: Arc<MockProvider>,
    router: axum::Router,
    project: Id,
}

async fn harness() -> Harness {
    let store = Store::in_memory().unwrap();
    let provider = Arc::new(MockProvider::new());
    let module = Relay::with_providers(vec![provider.clone()]);
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    assert_eq!(module.info().prefix, "balancers");
    module.start(ctx.clone()).await.unwrap();
    let client = module.client(&ctx);
    let router = module.router(ctx.clone());
    Harness {
        ctx,
        client,
        provider,
        router,
        project: Id::new(kind::PROJECT),
    }
}

impl Harness {
    async fn call(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        self.call_h(method, path, body, &[]).await
    }

    async fn call_h(
        &self,
        method: &str,
        path: &str,
        body: Option<Value>,
        headers: &[(&str, &str)],
    ) -> (StatusCode, Value) {
        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
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
            serde_json::from_slice(&bytes).unwrap()
        };
        (status, v)
    }

    /// Send a body the router may refuse to even parse, and keep whatever
    /// came back as text.
    async fn raw_body(&self, method: &str, path: &str, body: &str) -> (StatusCode, Value) {
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap();
        let mut req = req;
        req.extensions_mut().insert(admin_caller());
        let res = self.router.clone().oneshot(req).await.unwrap();
        let status = res.status();
        let bytes = axum::body::to_bytes(res.into_body(), 1 << 20)
            .await
            .unwrap();
        let text = String::from_utf8_lossy(&bytes).into_owned();
        (status, Value::String(text))
    }

    /// Read a balancer's `provider_ref`: the socket its provider really bound.
    fn provider_ref(&self, id: &Id) -> Option<String> {
        self.client.balancers().get(id).unwrap().status.provider_ref
    }

    /// Wait until `pred` holds for the balancer.
    async fn wait_lb(&self, id: &Id, pred: impl Fn(&Balancer) -> bool) -> Balancer {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let lb = self.client.balancers().get(id).unwrap();
            if pred(&lb) {
                return lb;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out: {lb:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_listener(&self, id: &Id, pred: impl Fn(&Listener) -> bool) -> Listener {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let l = self.client.listeners().get(id).unwrap();
            if pred(&l) {
                return l;
            }
            assert!(tokio::time::Instant::now() < deadline, "timed out: {l:?}");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    async fn wait_calls(&self, pred: impl Fn(&[Call]) -> bool) -> Vec<Call> {
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        loop {
            let calls = self.provider.calls();
            if pred(&calls) {
                return calls;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "timed out: {calls:?}"
            );
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }
}

fn id(v: &Value) -> Id {
    serde_json::from_value(v["id"].clone()).unwrap()
}

#[tokio::test]
async fn crud_over_http_drives_the_provider() {
    let h = harness().await;
    let project = h.project.to_string();

    // Balancer: created Pending, reconciled to Ready with a VIP.
    let (st, lb) = h
        .call(
            "POST",
            "/balancers",
            Some(json!({ "name": "web", "project": project, "spec": { "vip_network": "tenant" } })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{lb}");
    assert_eq!(lb["status"]["phase"], "pending");
    let lb_id = id(&lb);
    let lb = h.wait_lb(&lb_id, |b| b.status.phase == Phase::Ready).await;
    assert!(lb.status.vip.is_some());
    assert!(lb.status.provider_ref.is_some());
    let calls = h.provider.calls();
    assert!(matches!(&calls[0], Call::EnsureBalancer { balancer, .. } if *balancer == lb_id));

    // Pool with a TCP monitor, two members.
    let (st, pool) = h
        .call(
            "POST",
            "/pools",
            Some(json!({ "name": "web-pool", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": "tcp", "algorithm": "round_robin",
                "health_monitor": { "kind": "tcp", "interval": 5, "timeout": 3, "rise": 1, "fall": 1 }
            }})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{pool}");
    let pool_id = id(&pool);
    let (st, m1) = h
        .call(
            "POST",
            &format!("/pools/{pool_id}/members"),
            Some(json!({ "address": "10.0.0.11", "port": 8080 })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{m1}");
    assert_eq!(
        m1["status"]["operating_status"], "offline",
        "monitored members start offline"
    );
    let (st, _) = h
        .call(
            "POST",
            &format!("/pools/{pool_id}/members"),
            Some(json!({ "address": "10.0.0.12", "port": 8080, "weight": 2 })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let (st, dup) = h
        .call(
            "POST",
            &format!("/pools/{pool_id}/members"),
            Some(json!({ "address": "10.0.0.12", "port": 8080 })),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT, "{dup}");
    let (_, list) = h
        .call("GET", &format!("/pools/{pool_id}/members"), None)
        .await;
    assert_eq!(list["total"], 2);
    let (_, flat) = h
        .call("GET", &format!("/members?pool={pool_id}"), None)
        .await;
    assert_eq!(flat["total"], 2);

    // Listener on 80 -> pool. The provider is programmed with no members
    // (both are Offline until the checker sees them).
    h.provider.clear();
    let (st, lsn) = h
        .call(
            "POST",
            "/listeners",
            Some(json!({ "name": "web-80", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": { "type": "tcp" }, "port": 80,
                "default_pool": pool_id.to_string()
            }})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{lsn}");
    let lsn_id = id(&lsn);
    h.wait_listener(&lsn_id, |l| l.status.phase == Phase::Ready)
        .await;
    let calls = h.provider.calls();
    assert!(
        matches!(&calls[0], Call::EnsureListener { listener, port: 80, pool: Some(p), members, .. }
            if *listener == lsn_id && *p == pool_id && members.is_empty()),
        "{calls:?}"
    );

    // Port clash and L7-on-OVN are refused.
    let (st, _) = h
        .call(
            "POST",
            "/listeners",
            Some(json!({ "name": "clash", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": { "type": "tcp" }, "port": 80 }})),
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let (st, _) = h
        .call(
            "POST",
            "/listeners",
            Some(json!({ "name": "http", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": { "type": "http" }, "port": 8080 }})),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);

    // A member coming Online (as the checker would write it) re-programs
    // the listener with that member.
    h.provider.clear();
    let m1_id = id(&m1);
    h.client
        .members()
        .modify(&m1_id, |m| {
            m.status.operating_status = OperatingStatus::Online;
            Ok(())
        })
        .unwrap();
    h.wait_calls(|c| {
        c.iter().any(
            |c| matches!(c, Call::EnsureListener { members, .. } if members == &["10.0.0.11:8080"]),
        )
    })
    .await;
    let pool_now = h.client.pools().get(&pool_id).unwrap();
    assert_eq!(pool_now.status.online, 1);
    assert_eq!(pool_now.status.members, 2);

    // Stale If-Match conflicts; a good one updates.
    let (st, _) = h
        .call_h(
            "PUT",
            &format!("/listeners/{lsn_id}"),
            Some(json!({ "spec": { "balancer": lb_id.to_string(), "protocol": { "type": "tcp" }, "port": 81,
                                  "default_pool": pool_id.to_string() } })),
            &[("if-match", "1")],
        )
        .await;
    assert_eq!(st, StatusCode::CONFLICT);
    let cur = h.client.listeners().get(&lsn_id).unwrap();
    let (st, upd) = h
        .call_h(
            "PUT",
            &format!("/listeners/{lsn_id}"),
            Some(json!({ "spec": { "balancer": lb_id.to_string(), "protocol": { "type": "tcp" }, "port": 81,
                                  "default_pool": pool_id.to_string() } })),
            &[("if-match", &cur.meta.revision.to_string())],
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{upd}");
    assert_eq!(upd["spec"]["port"], 81);
    h.wait_calls(|c| {
        c.iter()
            .any(|c| matches!(c, Call::EnsureListener { port: 81, .. }))
    })
    .await;

    // Pool in use cannot be deleted; member delete re-programs.
    let (st, _) = h.call("DELETE", &format!("/pools/{pool_id}"), None).await;
    assert_eq!(st, StatusCode::CONFLICT);
    h.provider.clear();
    let (st, _) = h
        .call("DELETE", &format!("/pools/{pool_id}/members/{m1_id}"), None)
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    h.wait_calls(|c| {
        c.iter()
            .any(|c| matches!(c, Call::EnsureListener { members, .. } if members.is_empty()))
    })
    .await;

    // Lists, filters, immutability.
    let (_, lbs) = h.call("GET", "/balancers?all=true", None).await;
    assert_eq!(lbs["total"], 1);
    let (_, lsns) = h
        .call("GET", &format!("/listeners?balancer={lb_id}"), None)
        .await;
    assert_eq!(lsns["total"], 1);
    let (st, _) = h
        .call(
            "PUT",
            &format!("/balancers/{lb_id}"),
            Some(json!({ "spec": { "vip_network": "other" } })),
        )
        .await;
    assert_eq!(st, StatusCode::BAD_REQUEST);
    let (st, by_name) = h
        .call(
            "GET",
            &format!("/balancers/web?project={}", h.project),
            None,
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{by_name}");

    // Cascade delete: listener and balancer go to the provider, pool and
    // members disappear.
    h.provider.clear();
    let (st, _) = h.call("DELETE", &format!("/balancers/{lb_id}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let calls = h.provider.calls();
    assert!(calls.contains(&Call::DeleteListener {
        balancer: lb_id.clone(),
        listener: lsn_id.clone()
    }));
    assert!(calls.contains(&Call::DeleteBalancer(lb_id.clone())));
    assert!(h.client.pools().list(None).unwrap().is_empty());
    assert!(h.client.members().list(None).unwrap().is_empty());
    let (st, _) = h.call("GET", &format!("/listeners/{lsn_id}"), None).await;
    assert_eq!(st, StatusCode::NOT_FOUND);

    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn provider_failure_lands_in_status_and_retries() {
    let h = harness().await;
    h.provider.set_failure(Some("ovn unreachable"));
    let lb = h
        .client
        .create_balancer(
            pistack_types::Meta::new(kind::BALANCER, "flaky", Some(h.project.clone())),
            BalancerSpec {
                vip_network: "tenant".into(),
                vip_address: Some("10.9.9.9".parse().unwrap()),
                ..BalancerSpec::default()
            },
        )
        .unwrap();
    let lb = h
        .wait_lb(&lb.meta.id, |b| b.status.phase == Phase::Error)
        .await;
    assert_eq!(lb.status.message, "ovn unreachable");
    // The failure is written once. Its own event must not start another
    // attempt, or a broken balancer writes its status without end.
    let event = pistack_types::Event {
        kind: kind::BALANCER.to_owned(),
        id: lb.meta.id.clone(),
        project: lb.meta.project.clone(),
        action: pistack_types::EventAction::Updated,
        revision: lb.meta.revision,
        at: lb.meta.updated_at,
    };
    for _ in 0..3 {
        h.client.handle_event(&event).await;
    }
    let settled = h.client.balancers().get(&lb.meta.id).unwrap();
    assert_eq!(settled.meta.revision, lb.meta.revision);
    h.provider.set_failure(None);
    h.client.reconcile_pending().await;
    let lb = h
        .wait_lb(&lb.meta.id, |b| b.status.phase == Phase::Ready)
        .await;
    assert_eq!(lb.status.vip, Some("10.9.9.9".parse().unwrap()));
    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn ensure_tcp_balancer_is_idempotent_and_converges_members() {
    let h = harness().await;
    let a: std::net::SocketAddr = "10.0.1.1:8080".parse().unwrap();
    let b: std::net::SocketAddr = "10.0.1.2:8080".parse().unwrap();
    let c: std::net::SocketAddr = "10.0.1.3:8080".parse().unwrap();

    let lb = h
        .client
        .ensure_tcp_balancer(&h.project, "svc-web", "tenant", 80, &[a, b])
        .await
        .unwrap();
    assert_eq!(lb.meta.labels.get(MANAGED_LABEL).unwrap(), "loom");
    let again = h
        .client
        .ensure_tcp_balancer(&h.project, "svc-web", "tenant", 80, &[a, b])
        .await
        .unwrap();
    assert_eq!(again.meta.id, lb.meta.id);
    let pools = h.client.pools_of(&lb.meta.id).unwrap();
    assert_eq!(pools.len(), 1);
    let listeners = h.client.listeners_of(&lb.meta.id).unwrap();
    assert_eq!(listeners.len(), 1);
    assert_eq!(listeners[0].spec.port, 80);
    let mut have: Vec<_> = h
        .client
        .members_of(&pools[0].meta.id)
        .unwrap()
        .iter()
        .map(|m| m.addr())
        .collect();
    have.sort();
    assert_eq!(have, vec![a, b]);

    // Members and port change: converge.
    h.client
        .ensure_tcp_balancer(&h.project, "svc-web", "tenant", 8080, &[b, c])
        .await
        .unwrap();
    let listeners = h.client.listeners_of(&lb.meta.id).unwrap();
    assert_eq!(listeners.len(), 1);
    assert_eq!(listeners[0].spec.port, 8080);
    let mut have: Vec<_> = h
        .client
        .members_of(&pools[0].meta.id)
        .unwrap()
        .iter()
        .map(|m| m.addr())
        .collect();
    have.sort();
    assert_eq!(have, vec![b, c]);

    // Another network with the same name is a conflict.
    let err = h
        .client
        .ensure_tcp_balancer(&h.project, "svc-web", "elsewhere", 80, &[a])
        .await
        .unwrap_err();
    assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);

    // The module's background reconciler leaves loom's balancer alone:
    // only the owner's own client programs it.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let untouched = h.client.balancers().get(&lb.meta.id).unwrap();
    assert_ne!(untouched.status.phase, Phase::Ready);
    assert!(
        !h.provider
            .calls()
            .iter()
            .any(|c| matches!(c, Call::EnsureBalancer { balancer, .. } if *balancer == lb.meta.id)),
        "module reconciler must not program a loom balancer"
    );
    h.client.reconcile_balancer(&lb.meta.id).await;
    h.wait_lb(&lb.meta.id, |b| b.status.phase == Phase::Ready)
        .await;
    h.client.delete_balancer(&lb.meta.id).await.unwrap();
    assert!(h.client.listeners().list(None).unwrap().is_empty());
    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn health_checker_flips_online_then_offline() {
    let h = harness().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let accept = tokio::spawn(async move {
        while let Ok((s, _)) = listener.accept().await {
            drop(s);
        }
    });

    let lb = h
        .client
        .create_balancer(
            pistack_types::Meta::new(kind::BALANCER, "hc", Some(h.project.clone())),
            BalancerSpec {
                vip_network: "tenant".into(),
                ..BalancerSpec::default()
            },
        )
        .unwrap();
    let pool = h
        .client
        .create_pool(
            pistack_types::Meta::new(kinds::POOL, "hc-pool", Some(h.project.clone())),
            PoolSpec {
                balancer: lb.meta.id.clone(),
                protocol: PoolProtocol::Tcp,
                algorithm: Algorithm::LeastConn,
                session_persistence: None,
                health_monitor: Some(HealthMonitor {
                    rise: 2,
                    fall: 2,
                    ..HealthMonitor::default()
                }),
                admin_up: true,
            },
        )
        .unwrap();
    let member = h
        .client
        .create_member(
            pistack_types::Meta::new(kinds::MEMBER, "hc-m", Some(h.project.clone())),
            MemberSpec {
                pool: pool.meta.id.clone(),
                address: addr.ip(),
                port: addr.port(),
                weight: 1,
                backup: false,
                admin_up: true,
            },
        )
        .unwrap();
    assert_eq!(member.status.operating_status, OperatingStatus::Offline);
    let lsn = h
        .client
        .create_listener(
            pistack_types::Meta::new(kinds::LISTENER, "hc-80", Some(h.project.clone())),
            ListenerSpec {
                balancer: lb.meta.id.clone(),
                protocol: ListenerProtocol::Tcp,
                port: 80,
                default_pool: Some(pool.meta.id.clone()),
                connection_limit: 0,
                admin_up: true,
                door: None,
                routes: Vec::new(),
            },
        )
        .unwrap();
    h.wait_listener(&lsn.meta.id, |l| l.status.phase == Phase::Ready)
        .await;

    // Drive the checker by hand. The module health loop may also tick; keep
    // forcing until Online so a concurrent streak does not flake the test.
    let mut checker = Checker::new(h.client.clone());
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let _ = checker.tick(true).await;
        let m = h.client.members().get(&member.meta.id).unwrap();
        if m.status.operating_status == OperatingStatus::Online {
            assert!(m.status.last_check.is_some());
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "member never went Online: {m:?}"
        );
    }
    let want = addr.to_string();
    h.wait_calls(|c| {
        c.iter().any(
            |c| matches!(c, Call::EnsureListener { members, .. } if members == std::slice::from_ref(&want)),
        )
    })
    .await;

    // Backend goes away: force until Offline.
    accept.abort();
    let _ = accept.await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    loop {
        let _ = checker.tick(true).await;
        let m = h.client.members().get(&member.meta.id).unwrap();
        if m.status.operating_status == OperatingStatus::Offline {
            assert!(
                m.status.message.starts_with("connect:"),
                "{}",
                m.status.message
            );
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "member never went Offline: {m:?}"
        );
    }
    h.wait_calls(|c| {
        c.iter()
            .rev()
            .find(|c| matches!(c, Call::EnsureListener { .. }))
            .is_some_and(
                |c| matches!(c, Call::EnsureListener { members, .. } if members.is_empty()),
            )
    })
    .await;
    let pool_now = h.client.pools().get(&pool.meta.id).unwrap();
    assert_eq!(pool_now.status.online, 0);

    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn every_kind_create_list_stale_update_delete() {
    let h = harness().await;
    let p = h.project.to_string();
    let (st, lb) = h
        .call(
            "POST",
            "/balancers",
            Some(json!({ "name": "k", "project": p, "spec": { "vip_network": "n" } })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let lb_id = id(&lb);
    let (st, pool) = h
        .call(
            "POST",
            "/pools",
            Some(json!({ "name": "k-p", "project": p, "spec": { "balancer": lb_id.to_string() } })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    let pool_id = id(&pool);
    let (st, m) = h
        .call(
            "POST",
            &format!("/pools/{pool_id}/members"),
            Some(json!({ "address": "10.0.0.1", "port": 1 })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED);
    assert_eq!(m["status"]["operating_status"], "no_monitor");
    let m_id = id(&m);

    for (path, body) in [
        (
            format!("/balancers/{lb_id}"),
            json!({ "spec": { "vip_network": "n" } }),
        ),
        (
            format!("/pools/{pool_id}"),
            json!({ "spec": { "balancer": lb_id.to_string(), "algorithm": "source_ip" } }),
        ),
        (
            format!("/pools/{pool_id}/members/{m_id}"),
            json!({ "address": "10.0.0.1", "port": 2 }),
        ),
    ] {
        let (st, _) = h
            .call_h("PUT", &path, Some(body.clone()), &[("if-match", "999")])
            .await;
        assert_eq!(st, StatusCode::CONFLICT, "{path}");
        let (st, v) = h.call("PUT", &path, Some(body)).await;
        assert_eq!(st, StatusCode::OK, "{path}: {v}");
    }
    let (_, v) = h
        .call("GET", &format!("/pools/{pool_id}/members/{m_id}"), None)
        .await;
    assert_eq!(v["spec"]["port"], 2);
    let (st, _) = h
        .call("DELETE", &format!("/pools/{pool_id}/members/{m_id}"), None)
        .await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = h.call("DELETE", &format!("/pools/{pool_id}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (st, _) = h.call("DELETE", &format!("/balancers/{lb_id}"), None).await;
    assert_eq!(st, StatusCode::NO_CONTENT);
    let (_, lbs) = h.call("GET", "/balancers?all=true", None).await;
    assert_eq!(lbs["total"], 0);
    h.ctx.shutdown.cancel();
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
    let app = Relay::default().router(ctx);
    let res = app
        .oneshot(
            Request::builder()
                .uri("/balancers")
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

#[tokio::test]
async fn listener_routes_are_validated_and_hold_their_pools() {
    use pistack_types::error::ErrorCode;
    let h = harness().await;
    let meta = |k: &'static str, n: &str| pistack_types::Meta::new(k, n, Some(h.project.clone()));
    let lb = h
        .client
        .create_balancer(
            meta(kind::BALANCER, "routed"),
            BalancerSpec {
                vip_network: "tenant".into(),
                provider: Provider::Relay,
                ..BalancerSpec::default()
            },
        )
        .unwrap();
    let pool = |name: &str, protocol: PoolProtocol| {
        h.client
            .create_pool(
                meta(kinds::POOL, name),
                PoolSpec {
                    balancer: lb.meta.id.clone(),
                    protocol,
                    algorithm: Algorithm::RoundRobin,
                    session_persistence: None,
                    health_monitor: None,
                    admin_up: true,
                },
            )
            .unwrap()
    };
    let default = pool("default", PoolProtocol::Http);
    let api = pool("api", PoolProtocol::Http);
    let udp = pool("udp", PoolProtocol::Udp);
    let route = |host: Option<&str>, path: &str, pool: &Id| Route {
        host: host.map(str::to_owned),
        path: path.into(),
        path_match: PathMatch::Prefix,
        pool: pool.clone(),
    };
    let spec = |protocol: ListenerProtocol, routes: Vec<Route>| ListenerSpec {
        balancer: lb.meta.id.clone(),
        protocol,
        port: 80,
        default_pool: Some(default.meta.id.clone()),
        connection_limit: 0,
        admin_up: true,
        door: None,
        routes,
    };
    let refused = [
        spec(
            ListenerProtocol::Tcp,
            vec![route(None, "/api", &api.meta.id)],
        ),
        spec(
            ListenerProtocol::Http,
            vec![route(None, "api", &api.meta.id)],
        ),
        spec(
            ListenerProtocol::Http,
            vec![route(Some("App.example.com"), "/", &api.meta.id)],
        ),
        spec(
            ListenerProtocol::Http,
            vec![route(Some("app:8080"), "/", &api.meta.id)],
        ),
        spec(ListenerProtocol::Http, vec![route(None, "/", &udp.meta.id)]),
    ];
    for bad in refused {
        let err = h
            .client
            .create_listener(meta(kinds::LISTENER, "bad"), bad.clone())
            .unwrap_err();
        assert_eq!(err.code, ErrorCode::BadRequest, "{bad:?}: {err}");
    }
    let lsn = h
        .client
        .create_listener(
            meta(kinds::LISTENER, "web"),
            spec(
                ListenerProtocol::Http,
                vec![route(Some("*.example.com"), "/api", &api.meta.id)],
            ),
        )
        .unwrap();
    assert_eq!(
        h.client.listeners_using_pool(&api.meta.id).unwrap()[0]
            .meta
            .id,
        lsn.meta.id
    );
    let err = h.client.delete_pool(&api.meta.id).unwrap_err();
    assert_eq!(err.code, ErrorCode::Conflict);
    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn a_pool_algorithm_is_named_or_refused() {
    let h = harness().await;
    let project = h.project.to_string();
    let (st, lb) = h
        .call(
            "POST",
            "/balancers",
            Some(json!({ "name": "alg", "project": project, "spec": { "vip_network": "tenant" } })),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{lb}");
    let lb_id = id(&lb);

    for name in ["round_robin", "least_conn", "source_ip"] {
        let (st, pool) = h
            .call(
                "POST",
                "/pools",
                Some(
                    json!({ "name": format!("p-{name}"), "project": project, "spec": {
                        "balancer": lb_id.to_string(), "protocol": "tcp", "algorithm": name
                    }}),
                ),
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "{name}: {pool}");
        assert_eq!(pool["spec"]["algorithm"], name);
    }

    // Anything else is refused on the way in, naming what may be used. The
    // pool is read through its spec, so the refusal is the body's own.
    let (st, raw) = h.raw_body(
        "POST",
        "/pools",
        &format!(
            r#"{{"name":"p-bad","project":"{project}","spec":{{"balancer":"{lb_id}","protocol":"tcp","algorithm":"weighted_rr"}}}}"#,
        ),
    )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{raw}");
    let message = raw.to_string();
    assert!(message.contains("weighted_rr"), "{message}");
    for name in ["round_robin", "least_conn", "source_ip"] {
        assert!(message.contains(name), "{message}");
    }

    let (st, pool) = h
        .call(
            "POST",
            "/pools",
            Some(json!({ "name": "p-upd", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": "tcp", "algorithm": "least_conn"
            }})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{pool}");
    let pool_id = id(&pool);

    // The same refusal on update, with the pool left as it was.
    let (st, v) = h
        .raw_body(
            "PUT",
            &format!("/pools/{pool_id}"),
            &format!(r#"{{"spec":{{"balancer":"{lb_id}","protocol":"tcp","algorithm":"hash"}}}}"#),
        )
        .await;
    assert_eq!(st, StatusCode::UNPROCESSABLE_ENTITY, "{v}");
    assert!(v.to_string().contains("hash"), "{v}");
    let (_, kept) = h.call("GET", &format!("/pools/{pool_id}"), None).await;
    assert_eq!(kept["spec"]["algorithm"], "least_conn");

    let (st, v) = h
        .call(
            "PUT",
            &format!("/pools/{pool_id}"),
            Some(json!({ "spec": {
                "balancer": lb_id.to_string(), "protocol": "tcp", "algorithm": "source_ip"
            }})),
        )
        .await;
    assert_eq!(st, StatusCode::OK, "{v}");
    assert_eq!(v["spec"]["algorithm"], "source_ip");
    h.ctx.shutdown.cancel();
}

/// The pool's algorithm reaching a socket that is really forwarding, with
/// the API, the reconciler and a member health check all in between.
async fn data_plane(algorithm: &str, vip: std::net::IpAddr) -> (Harness, Id) {
    data_plane_at(algorithm, vip, 9401, 9402).await
}

/// As above with the two members on the ports given. Each data-plane test
/// picks its own pair: they run side by side, and a port is claimed by the
/// member that answers on it.
async fn data_plane_at(
    algorithm: &str,
    vip: std::net::IpAddr,
    heavy_port: u16,
    light_port: u16,
) -> (Harness, Id) {
    let store = Store::in_memory().unwrap();
    let module = Relay::forward();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    module.start(ctx.clone()).await.unwrap();
    let router = module.router(ctx.clone());
    let client = module.client(&ctx);
    let project = Id::new(kind::PROJECT);
    let h = Harness {
        ctx,
        client,
        provider: Arc::new(MockProvider::new()),
        router,
        project: project.clone(),
    };
    let project = project.to_string();
    let (_, lb) = h
        .call(
            "POST",
            "/balancers",
            Some(json!({ "name": "live", "project": project, "spec": {
                "vip_network": "tenant", "vip_address": vip.to_string()
            }})),
        )
        .await;
    let lb_id = id(&lb);
    let (_, pool) = h
        .call(
            "POST",
            "/pools",
            Some(json!({ "name": "live-pool", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": "tcp", "algorithm": algorithm
            }})),
        )
        .await;
    let pool_id = id(&pool);
    for (n, port) in [("heavy", heavy_port), ("light", light_port)] {
        let (st, m) = h
            .call(
                "POST",
                &format!("/pools/{pool_id}/members"),
                Some(json!({ "name": n, "address": "127.0.0.1", "port": port,
                             "weight": if n == "heavy" { 3 } else { 1 } })),
            )
            .await;
        assert_eq!(st, StatusCode::CREATED, "{m}");
    }
    let (st, lsn) = h
        .call(
            "POST",
            "/listeners",
            Some(json!({ "name": "live-9400", "project": project, "spec": {
                "balancer": lb_id.to_string(), "protocol": { "type": "tcp" },
                "port": 9400, "default_pool": pool_id.to_string()
            }})),
        )
        .await;
    assert_eq!(st, StatusCode::CREATED, "{lsn}");
    // Nothing is monitored: both members read `no_monitor`, so the pool
    // counts two of them online once the balancer is reconciled.
    let listener_id = id(&lsn);
    h.wait_listener(&listener_id, |l| l.status.phase == Phase::Ready)
        .await;
    h.wait_lb(&lb_id, |b| b.status.phase == Phase::Ready).await;
    for _ in 0..25 {
        h.client.reconcile_balancer(&lb_id).await;
        h.client.reconcile_listener(&listener_id).await;
        h.client.reconcile_pool(&pool_id).await;
        let pool_row = h.client.pools().get(&pool_id).unwrap();
        if pool_row.status.members == 2 {
            return (h, lb_id);
        }
        tokio::time::sleep(Duration::from_millis(40)).await;
    }
    let pool_row = h.client.pools().get(&pool_id).unwrap();
    panic!(
        "the listener was never programmed with both members: {:?}",
        pool_row.status
    );
}

/// A member that answers each connection with its own name.
async fn member(port: u16, body: &'static [u8]) -> tokio::task::JoinHandle<()> {
    answer(member_listener(port).await, body).await
}

/// A member that names itself and then keeps the connection open. What it
/// holds is the load `least_conn` counts.
async fn silent_member(port: u16, keep: &'static [u8]) -> tokio::task::JoinHandle<()> {
    let listener = member_listener(port).await;
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            tokio::spawn(async move {
                let mut buf = [0u8; 4];
                if sock.read_exact(&mut buf).await.is_err() {
                    return;
                }
                // The client sees who answered, then leaves the connection
                // standing. That is the load `least_conn` has to count.
                let _ = sock.write_all(keep).await;
                let _ = sock.readable().await;
                tokio::time::sleep(Duration::from_secs(120)).await;
            });
        }
    })
}

async fn member_listener(port: u16) -> tokio::net::TcpListener {
    match tokio::time::timeout(
        std::time::Duration::from_secs(2),
        tokio::net::TcpListener::bind(("127.0.0.1", port)),
    )
    .await
    {
        Ok(Ok(l)) => l,
        _ => panic!("port {port} is in use; the data-plane tests need it free"),
    }
}

async fn answer(
    listener: tokio::net::TcpListener,
    body: &'static [u8],
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 4];
            if sock.read_exact(&mut buf).await.is_err() {
                continue;
            }
            let _ = sock.write_all(body).await;
        }
    })
}

async fn through_vip(at: &str, n: usize) -> Vec<Vec<u8>> {
    let mut seen = Vec::new();
    for _ in 0..n {
        let mut client = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            TcpStream::connect(at.parse::<std::net::SocketAddr>().unwrap()),
        )
        .await
        .expect("timed out connecting to the vip")
        .unwrap();
        use tokio::io::AsyncWriteExt;
        client.write_all(b"ping").await.unwrap();
        let mut buf = vec![0u8; 4];
        tokio::time::timeout(
            std::time::Duration::from_secs(5),
            client.read_exact(&mut buf),
        )
        .await
        .expect("timed out waiting for a member")
        .unwrap();
        seen.push(buf);
        drop(client);
    }
    seen
}

#[tokio::test]
async fn a_tcp_listener_spreads_connections_by_the_pools_algorithm() {
    let vip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    let _heavy = member(9401, b"heav").await;
    let _light = member(9402, b"ligh").await;
    let (h, lb) = data_plane("round_robin", vip).await;
    let at = h
        .provider_ref(&lb)
        .expect("the forwarder reports its socket");
    let names: Vec<Vec<u8>> = through_vip(&at, 8).await;
    let heavy = names.iter().filter(|b| b.as_slice() == b"heav").count();
    assert_eq!(heavy, 6, "weight 3 against weight 1: {names:?}");
    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn a_tcp_listener_honours_a_source_ip_pool() {
    let vip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    let _heavy = member(9413, b"heav").await;
    let _light = member(9414, b"ligh").await;
    let (h, lb) = data_plane_at("source_ip", vip, 9413, 9414).await;
    let at = h
        .provider_ref(&lb)
        .expect("the forwarder reports its socket");
    let seen = through_vip(&at, 6).await;
    assert!(
        seen.windows(2).all(|w| w[0] == w[1]),
        "one client address moved between members: {seen:?}"
    );
    h.ctx.shutdown.cancel();
}

#[tokio::test]
async fn a_tcp_listener_honours_a_least_conn_pool() {
    let vip = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);
    // Both members carry the same weight, so only what is open on them can
    // tell the two apart. One keeps every connection it is given, the other
    // answers at once and lets go.
    let _slow = silent_member(9411, b"slow").await;
    let _fast = member(9412, b"fast").await;
    let (h, lb) = data_plane_at("least_conn", vip, 9411, 9412).await;
    let at = h
        .provider_ref(&lb)
        .expect("the forwarder reports its socket");

    // Three connections arrive while the first is still standing. Whoever
    // the first landed on, it is the busy one from then on, so the other two
    // have to go to the other member: a balancer still walking its members
    // in turn would hand the third connection back to the busy one.
    let seen = through_vip(&at, 3).await;
    let slow = seen.iter().filter(|b| b.as_slice() == b"slow").count();
    assert_eq!(
        slow, 1,
        "a member holding a connection took another: {seen:?}"
    );
    // The held connection ends with the test; the member is only waiting
    // on it, so nothing here has to wait for it back.
    h.ctx.shutdown.cancel();
}
