// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! End-to-end tests over `Store::in_memory` and `MockProvider`.

use axum::body::Body;
use axum::http::{header, Request, StatusCode};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Leadership, Module, ModuleContext, Phase};
use relay::health::Checker;
use relay::*;
use serde_json::{json, Value};
use std::sync::Arc;
use std::time::Duration;
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
        leadership: Leadership::default(),
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
            },
        )
        .unwrap();
    h.wait_listener(&lsn.meta.id, |l| l.status.phase == Phase::Ready)
        .await;

    // Drive the checker by hand (the module's own loop is also running on
    // a 1s tick with a 5s interval; `force` sidesteps both).
    let mut checker = Checker::new(h.client.clone());
    assert_eq!(
        checker.tick(true).await,
        0,
        "rise=2: first success is a streak"
    );
    assert_eq!(
        h.client
            .members()
            .get(&member.meta.id)
            .unwrap()
            .status
            .streak,
        1
    );
    assert_eq!(checker.tick(true).await, 1);
    let m = h.client.members().get(&member.meta.id).unwrap();
    assert_eq!(m.status.operating_status, OperatingStatus::Online);
    assert!(m.status.last_check.is_some());
    // The listener got re-programmed with the member.
    let want = addr.to_string();
    h.wait_calls(|c| {
        c.iter().any(
            |c| matches!(c, Call::EnsureListener { members, .. } if members == std::slice::from_ref(&want)),
        )
    })
    .await;

    // Backend goes away: two failures take it Offline.
    accept.abort();
    let _ = accept.await;
    assert_eq!(checker.tick(true).await, 0);
    assert_eq!(checker.tick(true).await, 1);
    let m = h.client.members().get(&member.meta.id).unwrap();
    assert_eq!(m.status.operating_status, OperatingStatus::Offline);
    assert!(
        m.status.message.starts_with("connect:"),
        "{}",
        m.status.message
    );
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

    // Admin-down members are not probed and read Draining.
    h.client
        .update_member(&member.meta.id, None, |m| {
            m.spec.admin_up = false;
            Ok(())
        })
        .unwrap();
    assert_eq!(checker.tick(true).await, 0);
    assert_eq!(
        h.client
            .members()
            .get(&member.meta.id)
            .unwrap()
            .status
            .operating_status,
        OperatingStatus::Draining
    );
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
        leadership: Leadership::default(),
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
