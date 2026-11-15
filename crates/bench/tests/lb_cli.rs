// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi lb` end to end against the relay module itself: the real router over
//! an in-memory store and a mock provider, so the paths and bodies the CLI
//! builds are checked against the routes they are written for.

use axum::{
    body::Body,
    extract::{Path, Request},
    http::StatusCode,
    middleware,
    response::IntoResponse,
    routing::get,
    Json, Router,
};
use bench::cli::{self, Io};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use relay::{MockProvider, Relay};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::Arc;
use tower::ServiceExt;

const NET_ID: &str = "net-0199a5e0-1111-7000-8000-000000000004";

fn caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

/// The relay routes, plus a fake network endpoint so the CLI can resolve
/// `edge` the way it resolves every network.
/// The relay module, as the gateway mounts it: under its own prefix.
async fn relay_app(store: &Store) -> Router {
    let module = Relay::with_providers(vec![Arc::new(MockProvider::new())]);
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let nets = Router::new()
        .route("/network/networks/{id}", get(show_network))
        .route(
            "/network/networks",
            get(|| async { Json(json!({"items": [], "total": 0})) }),
        )
        // A network asked for by name is answered by id, as fabric answers it.
        .route("/network/networks/edge", get(edge_network));
    Router::new()
        .nest("/balancers", module.router(ctx))
        .merge(nets)
        .layer(middleware::from_fn(add_caller))
}

async fn add_caller(mut req: Request<Body>, next: middleware::Next) -> axum::response::Response {
    req.extensions_mut().insert(caller());
    next.run(req).await
}

async fn edge_network() -> axum::response::Response {
    show_network(Path(NET_ID.to_owned())).await
}

async fn show_network(Path(id): Path<String>) -> axum::response::Response {
    // Only the network this suite seeds has a name to give; anything else
    // is a network the CLI cannot read, and must fall back to its id.
    if id == NET_ID {
        return Json(json!({
            "id": NET_ID,
            "name": "edge",
            "kind": "network",
        }))
        .into_response();
    }
    (
        StatusCode::NOT_FOUND,
        Json(json!({"error": {"message": "no such network"}})),
    )
        .into_response()
}

struct Srv {
    app: Router,
    base: String,
    dir: tempfile::TempDir,
}

impl Srv {
    async fn get(&self, path: &str) -> (StatusCode, Value) {
        let res = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("GET")
                    .uri(path)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        read(res).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        let res = self
            .app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri(path)
                    .header(axum::http::header::CONTENT_TYPE, "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        read(res).await
    }
}

async fn read(res: axum::response::Response) -> (StatusCode, Value) {
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

async fn serve() -> Srv {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = Store::in_memory().unwrap();
    let app = relay_app(&store).await;
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Srv {
        app,
        base: format!("http://{addr}"),
        dir: tempfile::tempdir().unwrap(),
    }
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(s: &Srv, args: &[&str]) -> Run {
    let mut argv: Vec<String> = [
        "pi",
        "--config",
        s.dir.path().join("config.toml").to_str().unwrap(),
        "--url",
        &s.base,
        "--token",
        "t",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    argv.extend(args.iter().map(|s| s.to_string()));
    let mut input = Cursor::new(Vec::new());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut io = Io {
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut err,
            tty: false,
        };
        cli::run(argv, &mut io).await.expect("run")
    };
    Run {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

/// Create a balancer through the API, as an earlier command would have.
/// Returns its id.
async fn seed_lb(s: &Srv) -> String {
    let (status, v) = s
        .post(
            "/balancers/balancers",
            json!({"name": "web-lb", "spec": {"vip_network": NET_ID, "provider": "relay"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

/// ...and a pool under it. Returns `(balancer_id, pool_id)`.
async fn seed_pool(s: &Srv) -> (String, String) {
    let lb = seed_lb(s).await;
    let (status, v) = s
        .post(
            "/balancers/pools",
            json!({"name": "web-pool", "spec": {"balancer": lb}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    (lb, v["id"].as_str().unwrap().to_owned())
}

fn member_name(pool_name: &str, addr: &str, port: u16) -> String {
    format!("{pool_name}-{addr}-{port}").replace(':', "-")
}

#[tokio::test]
async fn lb_create_list_show_delete() {
    let s = serve().await;

    let r = pi(&s, &["lb", "create", "web-lb", "--network", "edge"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("lb \"web-lb\" created"), "{}", r.out);

    // The network name became its id in the spec.
    let r = pi(&s, &["-o", "json", "lb", "show", "web-lb"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert!(v["id"].as_str().unwrap().starts_with("lb-"));
    assert_eq!(v["spec"]["vip_network"], NET_ID);
    assert_eq!(v["spec"]["provider"], "forward");
    assert_eq!(v["spec"]["admin_up"], true);

    let r = pi(&s, &["lb", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<&str> = r.out.lines().collect();
    assert!(
        lines[0].contains("NAME") && lines[0].contains("VIP"),
        "{}",
        r.out
    );
    assert!(lines[1].starts_with("web-lb"), "{}", r.out);

    // vip, port, provider and --down reach the spec.
    let r = pi(
        &s,
        &[
            "lb",
            "create",
            "vip-lb",
            "--network",
            NET_ID,
            "--vip",
            "10.9.0.5",
            "--vip-port",
            "8443",
            "--provider",
            "relay",
            "--down",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["-o", "json", "lb", "show", "vip-lb"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["spec"]["vip_address"], "10.9.0.5");
    assert_eq!(v["spec"]["vip_port"], 8443);
    assert_eq!(v["spec"]["provider"], "relay");
    assert_eq!(v["spec"]["admin_up"], false);

    // An unusable vip is a usage error; no request is sent.
    let r = pi(
        &s,
        &["lb", "create", "bad", "--network", "edge", "--vip", "nope"],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("not an IP address"), "{}", r.err);

    let r = pi(&s, &["lb", "list", "-o", "name"]).await;
    assert_eq!(r.out, "web-lb\nvip-lb\n", "{}", r.out);

    let r = pi(&s, &["lb", "delete", "web-lb", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("lb \"web-lb\" deleted"), "{}", r.out);

    // Without --yes there is no terminal to confirm on, so it refuses.
    let r = pi(&s, &["lb", "delete", "web-lb"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("without --yes"), "{}", r.err);

    let r = pi(&s, &["lb", "show", "web-lb"]).await;
    assert_ne!(r.code, 0);
}

#[tokio::test]
async fn listener_list_create_show_delete() {
    let s = serve().await;
    seed_lb(&s).await;

    let r = pi(
        &s,
        &[
            "lb",
            "listener",
            "create",
            "--balancer",
            "web-lb",
            "--port",
            "80",
            "http",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("created"), "{}", r.out);

    let r = pi(&s, &["-o", "json", "lb", "listener", "show", "http"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert!(v["id"].as_str().unwrap().starts_with("lsn-"));
    assert_eq!(v["spec"]["protocol"], json!({"type": "tcp"}));
    assert_eq!(v["spec"]["port"], 80);

    // Scoped to a balancer by name, and the other formats.
    let r = pi(&s, &["lb", "listener", "list", "--balancer", "web-lb"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("http"), "{}", r.out);
    let r = pi(&s, &["-o", "wide", "lb", "listener", "list"]).await;
    assert!(r.out.contains("FULL-ID"), "{}", r.out);
    let r = pi(&s, &["-o", "yaml", "lb", "listener", "list"]).await;
    assert!(r.out.contains("name: http"), "{}", r.out);
    let r = pi(&s, &["lb", "listener", "list", "-o", "json"]).await;
    let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(items.len(), 1);

    // A listener that binds nothing: no request, clap usage error.
    let r = pi(
        &s,
        &["lb", "listener", "create", "--balancer", "web-lb", "noport"],
    )
    .await;
    assert_eq!(r.code, 2, "{r:?}");
    // The balancer has no pool: it cannot be given one.
    let r = pi(
        &s,
        &[
            "lb",
            "listener",
            "create",
            "--balancer",
            "web-lb",
            "--port",
            "80",
            "--pool",
            "ghost",
            "http",
        ],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("pool 'ghost' not found"), "{}", r.err);
    // Terminated needs its certificate.
    let r = pi(
        &s,
        &[
            "lb",
            "listener",
            "create",
            "--balancer",
            "web-lb",
            "--port",
            "443",
            "--protocol",
            "terminated",
            "tls",
        ],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("--cert"), "{}", r.err);

    let r = pi(&s, &["lb", "listener", "delete", "http", "-y"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("deleted"), "{}", r.out);
    let r = pi(&s, &["lb", "listener", "show", "http"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn pool_list_create_show_delete() {
    let s = serve().await;
    seed_lb(&s).await;

    let r = pi(
        &s,
        &[
            "lb",
            "pool",
            "create",
            "--balancer",
            "web-lb",
            "--protocol",
            "http",
            "--algorithm",
            "least-conn",
            "--health-check",
            "--monitor",
            "http",
            "web-pool",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("created"), "{}", r.out);

    let r = pi(&s, &["-o", "json", "lb", "pool", "show", "web-pool"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert!(v["id"].as_str().unwrap().starts_with("pool-"));
    assert_eq!(v["spec"]["protocol"], "http");
    assert_eq!(v["spec"]["algorithm"], "least_conn");
    assert_eq!(v["spec"]["health_monitor"]["kind"], "http");
    assert_eq!(v["spec"]["health_monitor"]["interval"], 5);
    assert_eq!(v["spec"]["health_monitor"]["path"], "/");

    // An http pool probed over a bare connect: the monitor follows the
    // protocol unless --monitor says otherwise. No --health-check, no
    // monitor.
    let r = pi(
        &s,
        &[
            "lb",
            "pool",
            "create",
            "--balancer",
            "web-lb",
            "--protocol",
            "http",
            "--health-check",
            "plain-pool",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["-o", "json", "lb", "pool", "show", "plain-pool"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["spec"]["health_monitor"]["kind"], "http");
    let r = pi(
        &s,
        &[
            "lb",
            "pool",
            "create",
            "--balancer",
            "web-lb",
            "--protocol",
            "udp",
            "quiet-pool",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["-o", "json", "lb", "pool", "show", "quiet-pool"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert!(v["spec"].get("health_monitor").is_none(), "{}", v);

    // A tcp pool probed over http needs an explicit --monitor.
    let r = pi(
        &s,
        &[
            "lb",
            "pool",
            "create",
            "--balancer",
            "web-lb",
            "--health-check",
            "--monitor",
            "http",
            "probe-pool",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["-o", "json", "lb", "pool", "show", "probe-pool"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["spec"]["protocol"], "tcp");
    assert_eq!(v["spec"]["health_monitor"]["kind"], "http");

    let r = pi(
        &s,
        &["lb", "pool", "list", "-o", "name", "--balancer", "web-lb"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let mut names: Vec<&str> = r.out.lines().collect();
    names.sort();
    assert_eq!(
        names,
        ["plain-pool", "probe-pool", "quiet-pool", "web-pool"],
        "{}",
        r.out
    );

    let r = pi(&s, &["lb", "pool", "list", "--balancer", "web-lb"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("web-pool"), "{}", r.out);
    assert!(r.out.contains("least_conn"), "{}", r.out);

    // A pool a listener uses cannot go away; the server's conflict surfaces.
    let r = pi(
        &s,
        &[
            "lb",
            "listener",
            "create",
            "--balancer",
            "web-lb",
            "--port",
            "80",
            "--pool",
            "web-pool",
            "http",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["lb", "pool", "delete", "web-pool", "-y"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("used by listener"), "{}", r.err);

    let r = pi(&s, &["lb", "listener", "delete", "http", "-y"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["lb", "pool", "delete", "web-pool", "-y"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("deleted"), "{}", r.out);

    let r = pi(&s, &["lb", "pool", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
}

#[tokio::test]
async fn member_add_list_remove() {
    let s = serve().await;
    let (_lb, pool) = seed_pool(&s).await;

    let r = pi(
        &s,
        &[
            "lb",
            "member",
            "add",
            "--pool",
            "web-pool",
            "--port",
            "8080",
            "--weight",
            "3",
            "10.1.0.11",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("added"), "{}", r.out);

    // A second backend, IPv6; the list shows both as address:port.
    let r = pi(
        &s,
        &[
            "lb", "member", "add", "--pool", "web-pool", "--port", "8080", "--backup", "fd00::12",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&s, &["lb", "member", "list", "--pool", "web-pool"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("10.1.0.11:8080"), "{}", r.out);
    assert!(r.out.contains("[fd00::12]:8080"), "{}", r.out);
    assert!(r.out.contains("true"), "{}", r.out);
    let r = pi(&s, &["lb", "member", "list", "--pool", &pool, "-o", "json"]).await;
    let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(items.len(), 2, "{}", r.out);

    // `--only` keeps one member in the table.
    let r = pi(
        &s,
        &[
            "lb", "member", "list", "--pool", "web-pool", "--only", "fd00::12",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("fd00::12"), "{}", r.out);
    assert!(!r.out.contains("10.1.0.11"), "{}", r.out);
    assert_eq!(items[0]["spec"]["weight"], 3);
    assert!(items[0]["id"].as_str().unwrap().starts_with("mbr-"));

    // Bad address and an over-weight are usage errors.
    let r = pi(
        &s,
        &[
            "lb",
            "member",
            "add",
            "--pool",
            "web-pool",
            "--port",
            "1",
            "not-an-ip",
        ],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("not an IP address"), "{}", r.err);
    let r = pi(
        &s,
        &[
            "lb",
            "member",
            "add",
            "--pool",
            "web-pool",
            "--port",
            "1",
            "--weight",
            "257",
            "10.1.0.13",
        ],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("0-256"), "{}", r.err);

    // Remove by name, then by id.
    let name = member_name("web-pool", "10.1.0.11", 8080);
    let r = pi(
        &s,
        &["lb", "member", "remove", "--pool", "web-pool", &name, "-y"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("removed"), "{}", r.out);
    let id = {
        let r = pi(&s, &["lb", "member", "list", "-o", "json", "--pool", &pool]).await;
        let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
        items[0]["id"].as_str().unwrap().to_owned()
    };
    let r = pi(&s, &["lb", "member", "remove", "--pool", &pool, &id, "-y"]).await;
    assert_eq!(r.code, 0, "{r:?}");

    let r = pi(&s, &["lb", "member", "list", "-o", "json", "--pool", &pool]).await;
    assert_eq!(r.out.trim(), "[]", "{}", r.out);

    // --only belongs to member lists.
    let r = pi(&s, &["lb", "pool", "list", "--only", "web"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("--only"), "{}", r.err);
    let r = pi(&s, &["lb", "member", "list", "--pool", &pool, "-o", "wide"]).await;
    assert!(r.out.contains("FULL-ID"), "{}", r.out);

    // Unknown member name: a usage error that names it.
    let r = pi(
        &s,
        &[
            "lb", "member", "remove", "--pool", "web-pool", "ghost", "-y",
        ],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("'ghost' not found"), "{}", r.err);
}

/// Add one backend and hand back `(pool id, member id, member name)`.
async fn seed_member(s: &Srv, pool_name: &str, addr: &str, port: &str) -> (String, String, String) {
    let (_lb, pool) = seed_pool(s).await;
    let r = pi(
        s,
        &[
            "lb", "member", "add", "--pool", pool_name, "--port", port, addr,
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(
        s,
        &["lb", "member", "list", "-o", "json", "--pool", pool_name],
    )
    .await;
    let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(items.len(), 1, "{}", r.out);
    let id = items[0]["id"].as_str().unwrap().to_owned();
    let name = items[0]["name"].as_str().unwrap().to_owned();
    (pool, id, name)
}

async fn members_of(s: &Srv, pool: &str) -> Vec<Value> {
    let r = pi(s, &["lb", "member", "list", "-o", "json", "--pool", pool]).await;
    assert_eq!(r.code, 0, "{r:?}");
    serde_json::from_str(&r.out).unwrap()
}

async fn remove(s: &Srv, pool: &str, member: &str) -> Run {
    pi(s, &["lb", "member", "remove", "--pool", pool, member, "-y"]).await
}

#[tokio::test]
async fn member_remove_finds_by_every_usable_form() {
    // The forms on their own, and each with the pool named by id instead.
    for pool_arg in ["web-pool", "id"] {
        for form in ["name", "id", "short", "addrport"] {
            let s = serve().await;
            let (pool, id, name) = seed_member(&s, "web-pool", "10.60.0.3", "80").await;
            let short = {
                let r = pi(&s, &["lb", "member", "list", "--pool", "web-pool"]).await;
                r.out
                    .lines()
                    .nth(1)
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_owned()
            };
            let term = match form {
                "name" => name,
                "id" => id,
                "short" => short,
                _ => "10.60.0.3:80".to_owned(),
            };
            let which = if pool_arg == "id" {
                pool.as_str()
            } else {
                "web-pool"
            };
            let r = remove(&s, which, &term).await;
            assert_eq!(r.code, 0, "{pool_arg}/{form}: {r:?}");
            assert!(r.out.contains("removed"), "{pool_arg}/{form}: {}", r.out);
            assert!(members_of(&s, &pool).await.is_empty(), "{pool_arg}/{form}");
        }
    }
}

#[tokio::test]
async fn member_remove_by_address_alone_when_unique() {
    // One member on the address, with the pool named both ways.
    for pool_arg in ["web-pool", "id"] {
        let s = serve().await;
        let (pool, _, _) = seed_member(&s, "web-pool", "10.60.0.3", "80").await;
        let which = if pool_arg == "id" {
            pool.as_str()
        } else {
            "web-pool"
        };
        let r = remove(&s, which, "10.60.0.3").await;
        assert_eq!(r.code, 0, "{pool_arg}: {r:?}");
        assert!(members_of(&s, &pool).await.is_empty(), "{pool_arg}");
    }

    // An address nothing answers to is still a usage error.
    let s = serve().await;
    let (_, _, _) = seed_member(&s, "web-pool", "10.60.0.3", "80").await;
    let r = remove(&s, "web-pool", "10.60.9.9").await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn member_remove_asks_for_a_port_when_an_address_is_ambiguous() {
    let s = serve().await;
    let (_lb, pool) = seed_pool(&s).await;
    for port in ["80", "8080"] {
        let r = pi(
            &s,
            &[
                "lb",
                "member",
                "add",
                "--pool",
                "web-pool",
                "--port",
                port,
                "10.60.0.3",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{r:?}");
    }
    let r = remove(&s, "web-pool", "10.60.0.3").await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("ADDRESS:PORT"), "{}", r.err);
    assert!(r.err.contains("10.60.0.3:80"), "{}", r.err);
    assert!(r.err.contains("10.60.0.3:8080"), "{}", r.err);
    // Both are still there; the port picks one.
    let r = remove(&s, "web-pool", "10.60.0.3:8080").await;
    assert_eq!(r.code, 0, "{r:?}");
    let left = members_of(&s, &pool).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["spec"]["port"], 80);
    // The same backend under another name is still not found.
    let r = remove(&s, "web-pool", "10.60.0.3:9090").await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn member_remove_finds_an_ipv6_endpoint() {
    let s = serve().await;
    let (_lb, pool) = seed_pool(&s).await;
    for port in ["80", "8080"] {
        let r = pi(
            &s,
            &[
                "lb", "member", "add", "--pool", "web-pool", "--port", port, "fd00::3",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{r:?}");
    }
    // Address alone cannot say which of the two, bracketed or not. A bare
    // `fd00::3:80` is a real address of its own, so it names nothing; the
    // bracketed endpoint says the port.
    let r = remove(&s, "web-pool", "fd00::3").await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("ADDRESS:PORT"), "{}", r.err);
    assert!(r.err.contains("[fd00::3]:80"), "{}", r.err);
    let r = remove(&s, "web-pool", "fd00::3:80").await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("not found"), "{}", r.err);
    let r = remove(&s, "web-pool", "[fd00::3]:80").await;
    assert_eq!(r.code, 0, "{r:?}");
    let left = members_of(&s, &pool).await;
    assert_eq!(left.len(), 1, "{left:?}");
    assert_eq!(left[0]["spec"]["port"], 8080);
}

#[tokio::test]
async fn lb_tables_name_the_network() {
    let s = serve().await;
    let (_lb, _) = seed_pool(&s).await;

    let r = pi(&s, &["lb", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("edge"), "{}", r.out);
    assert!(!r.out.contains(NET_ID), "{}", r.out);
    let r = pi(&s, &["-o", "wide", "lb", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("edge"), "{}", r.out);
    assert!(!r.out.contains(NET_ID), "{}", r.out);

    // `show` prints the stored document either way, and `-o name` still
    // names the balancer.
    let r = pi(&s, &["lb", "show", "web-lb"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains(NET_ID), "{}", r.out);
    let r = pi(&s, &["lb", "list", "-o", "name"]).await;
    assert_eq!(r.out, "web-lb\n", "{}", r.out);

    // What is stored is untouched by what the table shows.
    let r = pi(&s, &["-o", "json", "lb", "list"]).await;
    let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(items[0]["spec"]["vip_network"], NET_ID);
    let r = pi(&s, &["-o", "yaml", "lb", "list"]).await;
    assert!(r.out.contains(NET_ID), "{}", r.out);
}

#[tokio::test]
async fn lb_table_falls_back_to_a_short_id_for_an_unknown_network() {
    let s = serve().await;
    let (status, v) = s
        .post(
            "/balancers/balancers",
            json!({"name": "far-lb", "spec": {"vip_network": "net-0199a5e0-1111-7000-8000-000000000099"}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let r = pi(&s, &["lb", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("net-00000099"), "{}", r.out);
}

#[tokio::test]
async fn delete_a_balancer_takes_its_pool_with_it() {
    // The CLI's balancer delete must reach the cascade the server documents.
    let s = serve().await;
    let (lb, pool) = seed_pool(&s).await;
    let r = pi(&s, &["lb", "delete", &lb, "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let (status, _) = s.get(&format!("/balancers/pools/{pool}")).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}
