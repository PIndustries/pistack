// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi vpc` and `pi vpc subnet` end to end against the fabric module itself:
//! the real router over an in-memory store, mounted under `/network` the way
//! the gateway mounts it, so the paths and bodies the CLI builds are checked
//! against the routes and types that actually answer them. Every refusal,
//! normalisation and carve in these tests is the module's own.

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware, Router,
};
use bench::cli::{self, Io};
use fabric::Fabric;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{Arc, Mutex};
use tower::ServiceExt;

fn caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "dev-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

/// The fabric routes as the gateway mounts them: the module's own router under
/// its `/network` prefix, with the caller the gateway would have attached.
///
/// `seen` collects every request the CLI put on the wire — method, path and
/// body — so a test can ask what actually went out, not only what came back.
async fn fabric_app(store: &Store, seen: Capture) -> Router {
    let module = Fabric::default();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    Router::new()
        .nest("/network", module.router(ctx))
        .layer(middleware::from_fn(add_caller))
        .layer(middleware::from_fn_with_state(seen, capture))
}

/// A request the CLI sent, recorded before the module saw it.
#[derive(Debug, Clone)]
struct Seen {
    method: String,
    path: String,
    body: Value,
}

type Capture = Arc<Mutex<Vec<Seen>>>;

/// Records the method, path and JSON body of every request, then hands the
/// request on intact. Reading the body here and rebuilding it is the whole
/// point: the CLI's request is what is examined, not a value the test holds.
async fn capture(
    State(seen): State<Capture>,
    req: Request<Body>,
    next: middleware::Next,
) -> axum::response::Response {
    let (mut parts, body) = req.into_parts();
    let is_write = matches!(
        parts.method,
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::DELETE
    );
    if !is_write {
        return next.run(Request::from_parts(parts, body)).await;
    }
    match axum::body::to_bytes(body, 1 << 20).await {
        Ok(bytes) => {
            let body = if bytes.is_empty() {
                Value::Null
            } else {
                serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null)
            };
            let path = parts.uri.path().to_owned();
            let method = parts.method.as_str().to_owned();
            seen.lock().unwrap().push(Seen { method, path, body });
            // The caller header the resolve-by-name path does not need but the
            // module does: re-attach nothing, the caller layer still runs.
            let _ = &mut parts;
            next.run(Request::from_parts(parts, Body::from(bytes)))
                .await
        }
        Err(_) => axum::response::Response::default(),
    }
}

async fn add_caller(mut req: Request<Body>, next: middleware::Next) -> axum::response::Response {
    req.extensions_mut().insert(caller());
    next.run(req).await
}

fn fabric(store: &Store) -> fabric::Client {
    fabric::Client::new(store.clone(), Arc::new(fabric::MockBackend::new()))
}

/// The suite's server: the real router, plus the store handle, so a test can
/// read back exactly what the module filed.
struct Srv {
    app: Router,
    base: String,
    dir: tempfile::TempDir,
    store: Store,
    seen: Capture,
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

impl Srv {
    async fn send(&self, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
        let req = Request::builder().method(method).uri(path);
        let req = match body {
            Some(b) => req
                .header(axum::http::header::CONTENT_TYPE, "application/json")
                .body(Body::from(b.to_string()))
                .unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let res = self.app.clone().oneshot(req).await.unwrap();
        read(res).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send("POST", path, Some(body)).await
    }

    /// The VPCs as the store holds them, keyed by resource name.
    fn vpcs(&self) -> Vec<(String, Value)> {
        named(fabric(&self.store).vpcs().list(None).unwrap())
    }

    fn vpc_rows(&self) -> Vec<Value> {
        self.vpcs().into_iter().map(|(_, v)| v).collect()
    }

    fn vpc_count(&self) -> usize {
        self.vpc_rows().len()
    }

    fn vpc(&self, name: &str) -> Value {
        find("vpc", &self.vpcs(), name)
    }

    /// The subnets as the store holds them, keyed by resource name.
    fn subnets(&self) -> Vec<(String, Value)> {
        named(fabric(&self.store).subnets().list(None).unwrap())
    }

    fn subnet_rows(&self) -> Vec<Value> {
        self.subnets().into_iter().map(|(_, s)| s).collect()
    }

    fn subnet_count(&self) -> usize {
        self.subnet_rows().len()
    }

    fn subnet(&self, name: &str) -> Value {
        find("subnet", &self.subnets(), name)
    }

    /// The write requests sent so far (POST/PUT/DELETE), oldest first.
    fn writes(&self) -> Vec<Seen> {
        self.seen.lock().unwrap().clone()
    }

    fn clear(&self) {
        self.seen.lock().unwrap().clear();
    }
}

fn find(what: &str, rows: &[(String, Value)], name: &str) -> Value {
    let mut names = Vec::new();
    for (n, v) in rows {
        if n == name {
            return v.clone();
        }
        names.push(n.clone());
    }
    panic!("no {what} named {name}; the store holds {names:?}")
}

fn named<K>(items: Vec<K>) -> Vec<(String, Value)>
where
    K: pistack_types::Kind + serde::de::DeserializeOwned,
{
    items
        .into_iter()
        .map(|k| {
            let name = k.meta().name.clone();
            (name, serde_json::to_value(k).unwrap())
        })
        .collect()
}

/// `vpc-0199a5e0-…-00000001` -> `vpc-00000001`, the CLI's own short form.
fn short(id: &str) -> String {
    let (prefix, uuid) = id.split_once('-').unwrap();
    let hex: String = uuid.chars().filter(|c| *c != '-').collect();
    let tail = hex.get(hex.len().saturating_sub(8)..).unwrap_or(&hex);
    format!("{prefix}-{tail}")
}

/// A network the CLI can resolve by name and hand to a VPC as its external
/// network. `vpc create` only asks the network to exist and be nameable; the
/// external/provider shape is the module's concern, not the resolve's.
async fn seed_network(s: &Srv, name: &str) -> String {
    let (status, v) = s
        .post(
            "/network/networks",
            json!({"name": name, "spec": {"external": true}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{status}: {v}");
    v["id"].as_str().unwrap().to_owned()
}

/// A VPC as an earlier `vpc create` would have left it. Returns its id.
async fn seed_vpc(s: &Srv, name: &str, cidrs: &[&str]) -> String {
    let (status, v) = s
        .post(
            "/network/vpcs",
            json!({"name": name, "spec": {"cidrs": cidrs}}),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

async fn serve() -> Srv {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = Store::in_memory().unwrap();
    let seen: Capture = Arc::new(Mutex::new(Vec::new()));
    let app = fabric_app(&store, seen.clone()).await;
    let served = app.clone();
    tokio::spawn(async move { axum::serve(listener, served).await.unwrap() });
    Srv {
        app,
        base: format!("http://{addr}"),
        dir: tempfile::tempdir().unwrap(),
        store,
        seen,
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

/// A rendered table, read by column.
struct Table {
    headers: Vec<String>,
    lines: Vec<String>,
}

impl Table {
    fn new(out: &str) -> Table {
        let lines: Vec<String> = out
            .lines()
            .map(|l| l.trim_end().to_owned())
            .filter(|l| !l.trim().is_empty())
            .collect();
        let headers = lines
            .first()
            .map(|h| h.split_whitespace().map(str::to_owned).collect())
            .unwrap_or_default();
        Table { headers, lines }
    }

    fn offset(&self, header: &str) -> usize {
        self.headers
            .iter()
            .position(|h| h == header)
            .unwrap_or_else(|| panic!("no {header} column: {:?}", self.headers));
        self.lines[0]
            .match_indices(header)
            .map(|(i, _)| i)
            .next()
            .unwrap_or_else(|| panic!("header {header} not on the header line"))
    }

    fn line(&self, name: &str) -> &str {
        for l in &self.lines[1..] {
            if l.split_whitespace().next() == Some(name) {
                return l;
            }
        }
        panic!("no row for {name}: {:?}", self.lines)
    }

    fn cell(&self, name: &str, header: &str) -> String {
        let from = self.offset(header);
        let line = self.line(name);
        let rest = line.get(from..).unwrap_or("");
        let next = self
            .headers
            .iter()
            .filter(|h| *h != header)
            .map(|h| self.offset(h))
            .filter(|o| *o > from)
            .min()
            .unwrap_or(usize::MAX);
        let width = next.saturating_sub(from);
        rest.get(..width).unwrap_or(rest).trim().to_owned()
    }

    fn count(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }
}

// ---- vpc list / show / delete --------------------------------------------------

#[tokio::test]
async fn vpc_list_and_show() {
    let s = serve().await;
    let prod = seed_vpc(&s, "prod", &["10.10.0.0/16"]).await;
    let dev = seed_vpc(&s, "dev", &["10.20.0.0/16", "10.21.0.0/16"]).await;
    s.clear();

    let r = pi(&s, &["vpc", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let t = Table::new(&r.out);
    assert_eq!(
        t.headers,
        [
            "NAME",
            "ID",
            "PHASE",
            "CIDRS",
            "EXTERNAL-NET",
            "SUBNETS",
            "ROUTER",
            "AGE"
        ]
    );
    assert_eq!(t.count(), 2);
    assert_eq!(t.cell("prod", "ID"), short(&prod));
    assert_eq!(t.cell("dev", "ID"), short(&dev));
    assert_eq!(t.cell("prod", "CIDRS"), "10.10.0.0/16");
    assert_eq!(t.cell("dev", "CIDRS"), "10.20.0.0/16,10.21.0.0/16");

    // The list hit `/network/vpcs`.
    assert!(
        s.writes().is_empty(),
        "a list writes nothing: {:?}",
        s.writes()
    );

    // Show by name and by id.
    for key in ["prod", &prod] {
        let r = pi(&s, &["vpc", "show", key]).await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
        assert!(r.out.contains("10.10.0.0/16"), "{}", r.out);
    }
    let r = pi(&s, &["vpc", "show", "prod", "-o", "json"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["id"], prod);
    assert_eq!(v["spec"]["cidrs"], json!(["10.10.0.0/16"]));

    let r = pi(&s, &["vpc", "show", "nope"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn vpc_delete_needs_yes_or_a_terminal() {
    let s = serve().await;
    let prod = seed_vpc(&s, "prod", &["10.10.0.0/16"]).await;

    // No terminal, no --yes: refused before anything is sent.
    let r = pi(&s, &["vpc", "delete", "prod"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert_eq!(s.vpc_count(), 1);

    let before = s.writes().len();
    let r = pi(&s, &["vpc", "delete", "prod", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "vpc \"prod\" deleted\n");
    assert_eq!(s.vpc_count(), 0);

    // The write was a DELETE on the VPC's own path, by name.
    let w = &s.writes()[before..];
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].method, "DELETE");
    assert_eq!(w[0].path, "/network/vpcs/prod");
    assert!(w[0].body.is_null(), "{:?}", w[0]);

    let _ = prod;
    let r = pi(&s, &["vpc", "delete", "prod", "--yes"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

// ---- vpc create ----------------------------------------------------------------

#[tokio::test]
async fn vpc_create_sends_the_cidrs_and_metadata() {
    let s = serve().await;
    let r = pi(
        &s,
        &[
            "vpc",
            "create",
            "prod",
            "--cidr",
            "10.10.0.0/16",
            "--cidr",
            "10.11.0.0/16",
            "--dns-domain",
            "prod.example",
            "--description",
            "production",
            "--label",
            "env=prod",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let created = s.vpc("prod");
    let id = created["id"].as_str().unwrap();
    assert!(
        r.out
            .starts_with(&format!("vpc \"prod\" created ({})", short(id))),
        "{}",
        r.out
    );
    assert_eq!(
        created["spec"]["cidrs"],
        json!(["10.10.0.0/16", "10.11.0.0/16"])
    );
    assert_eq!(created["spec"]["dns_domain"], "prod.example");
    assert_eq!(created["description"], "production");
    assert_eq!(created["labels"]["env"], "prod");
    // No --external-network: the field is absent, not null.
    assert!(
        created["spec"].get("external_network").is_none(),
        "{created}"
    );

    // The one write was a POST to `/network/vpcs` carrying that body.
    let w = s.writes();
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].method, "POST");
    assert_eq!(w[0].path, "/network/vpcs");
    assert_eq!(w[0].body["name"], "prod");
    assert_eq!(
        w[0].body["spec"]["cidrs"],
        json!(["10.10.0.0/16", "10.11.0.0/16"])
    );
    assert_eq!(w[0].body["spec"]["dns_domain"], "prod.example");
}

#[tokio::test]
async fn vpc_create_resolves_the_external_network_name_to_its_id() {
    let s = serve().await;
    let edge = seed_network(&s, "edge").await;
    s.clear();
    let r = pi(
        &s,
        &[
            "vpc",
            "create",
            "prod",
            "--cidr",
            "10.10.0.0/16",
            "--external-network",
            "edge",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let created = s.vpc("prod");
    // The VPC holds the network's id, not the name typed.
    assert_eq!(created["spec"]["external_network"], edge);

    // The resolve read `/network/networks/edge`, then posted the id.
    let w = s.writes();
    let post = w
        .iter()
        .find(|x| x.method == "POST")
        .expect("a POST create");
    assert_eq!(post.path, "/network/vpcs");
    assert_eq!(post.body["spec"]["external_network"], edge);
}

#[tokio::test]
async fn vpc_create_without_a_cidr_is_a_usage_error() {
    let s = serve().await;
    let r = pi(&s, &["vpc", "create", "prod"]).await;
    // clap rejects the missing required repeatable before any request goes out.
    assert_eq!(r.code, 2);
    assert!(r.err.contains("--cidr"), "{}", r.err);
    assert!(s.writes().is_empty(), "{:?}", s.writes());
    assert_eq!(s.vpc_count(), 0);
}

#[tokio::test]
async fn vpc_create_with_an_unknown_external_network_is_a_usage_error() {
    let s = serve().await;
    let r = pi(
        &s,
        &[
            "vpc",
            "create",
            "prod",
            "--cidr",
            "10.10.0.0/16",
            "--external-network",
            "ghost",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("network 'ghost' not found"), "{}", r.err);
    // Nothing was created: the resolve failed before the POST.
    assert_eq!(s.vpc_count(), 0);
    assert!(
        !s.writes().iter().any(|w| w.method == "POST"),
        "{:?}",
        s.writes()
    );
}

// ---- vpc subnet ----------------------------------------------------------------

#[tokio::test]
async fn vpc_subnet_list_hits_the_vpc_path_with_subnet_columns() {
    let s = serve().await;
    let vpc = seed_vpc(&s, "prod", &["10.10.0.0/16"]).await;
    // Two subnets cut from the VPC, made the way the shortcut makes them.
    for (name, cidr) in [("web", "10.10.1.0/24"), ("db", "10.10.2.0/24")] {
        let (status, v) = s
            .post(
                &format!("/network/vpcs/{vpc}/subnets"),
                json!({"name": name, "cidr": cidr}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
    }

    s.clear();
    let r = pi(&s, &["vpc", "subnet", "list", "prod"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let t = Table::new(&r.out);
    // The subnet kind's own columns, not a VPC-specific set.
    assert_eq!(
        t.headers,
        [
            "NAME",
            "ID",
            "PHASE",
            "NETWORK",
            "CIDR",
            "GATEWAY",
            "DHCP",
            "ALLOCATED",
            "AGE"
        ]
    );
    assert_eq!(t.count(), 2);
    assert_eq!(t.cell("web", "CIDR"), "10.10.1.0/24");
    assert_eq!(t.cell("db", "CIDR"), "10.10.2.0/24");

    // The VPC name was resolved to its id, and the list read under that id.
    let w = s.writes();
    // `subnet list` writes nothing; the only requests are the resolve (GET,
    // uncaptured) and the list (GET, uncaptured). Confirm no write leaked in.
    assert!(w.is_empty(), "a list writes nothing: {w:?}");
}

#[tokio::test]
async fn vpc_subnet_create_posts_name_and_cidr_under_the_vpc() {
    let s = serve().await;
    let vpc = seed_vpc(&s, "prod", &["10.10.0.0/16"]).await;

    s.clear();
    let r = pi(
        &s,
        &[
            "vpc",
            "subnet",
            "create",
            "prod",
            "web",
            "--cidr",
            "10.10.5.0/24",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let made = s.subnet("web");
    let id = made["id"].as_str().unwrap();
    assert!(
        r.out
            .starts_with(&format!("subnet \"web\" created ({})", short(id))),
        "{}",
        r.out
    );
    assert_eq!(made["spec"]["cidr"], "10.10.5.0/24");

    let w = s.writes();
    assert_eq!(w.len(), 1, "{w:?}");
    assert_eq!(w[0].method, "POST");
    // The VPC name was resolved, and the POST went to that VPC's id.
    assert_eq!(w[0].path, format!("/network/vpcs/{vpc}/subnets"));
    assert_eq!(w[0].body, json!({"name": "web", "cidr": "10.10.5.0/24"}));
}

#[tokio::test]
async fn vpc_subnet_create_accepts_a_bare_prefix_length() {
    let s = serve().await;
    seed_vpc(&s, "prod", &["10.10.0.0/16"]).await;
    s.clear();
    let r = pi(
        &s,
        &["vpc", "subnet", "create", "prod", "web", "--cidr", "24"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    // The module carves the next free /24 from the VPC's block.
    let made = s.subnet("web");
    let cidr = made["spec"]["cidr"].as_str().unwrap();
    assert!(cidr.ends_with("/24"), "{cidr}");
    assert!(cidr.starts_with("10.10."), "{cidr}");

    // The literal prefix is what the CLI put on the wire.
    let post = s.writes().into_iter().find(|w| w.method == "POST").unwrap();
    assert_eq!(post.body, json!({"name": "web", "cidr": "24"}));
}

#[tokio::test]
async fn vpc_subnet_delete_confirms_and_deletes_under_the_vpc() {
    let s = serve().await;
    let vpc = seed_vpc(&s, "prod", &["10.10.0.0/16"]).await;
    let sub = {
        let (status, v) = s
            .post(
                &format!("/network/vpcs/{vpc}/subnets"),
                json!({"name": "web", "cidr": "10.10.1.0/24"}),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
        v["id"].as_str().unwrap().to_owned()
    };
    assert_eq!(s.subnet_count(), 1);

    // No --yes, no terminal: refused, nothing removed.
    let r = pi(&s, &["vpc", "subnet", "delete", "prod", "web"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert_eq!(s.subnet_count(), 1);

    s.clear();
    let r = pi(&s, &["vpc", "subnet", "delete", "prod", "web", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "subnet \"web\" deleted\n");
    assert_eq!(s.subnet_count(), 0);

    // Both the VPC and the subnet were resolved to ids, and the DELETE carried
    // those ids under the VPC's path.
    let w = s.writes();
    let del = w.iter().find(|x| x.method == "DELETE").expect("a DELETE");
    assert_eq!(del.path, format!("/network/vpcs/{vpc}/subnets/{sub}"));
    assert!(del.body.is_null(), "{del:?}");
}

#[tokio::test]
async fn vpc_subnet_list_of_an_unknown_vpc_is_a_usage_error() {
    let s = serve().await;
    let r = pi(&s, &["vpc", "subnet", "list", "ghost"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("vpc 'ghost' not found"), "{}", r.err);
}
