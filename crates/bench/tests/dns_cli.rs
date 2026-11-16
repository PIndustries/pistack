// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi dns zone` and `pi dns record` end to end against the DNS module
//! itself: the real router over an in-memory store, mounted under `/dns` the
//! way the gateway mounts it, so the bodies the CLI builds are checked
//! against the types that actually answer them. There is no stand-in for the
//! service here — every refusal, normalisation and default in these tests is
//! the module's own.

use axum::{
    body::Body,
    extract::{Request, State},
    http::StatusCode,
    middleware, Router,
};
use beacon::{Beacon, Client as BeaconClient};
use bench::cli::{self, dns, Io};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Kind, Leadership, Module, ModuleContext, Phase};
use serde::de::DeserializeOwned;
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

/// The DNS routes as the gateway mounts them: the module's own router under
/// its own `/dns` prefix, with the caller the gateway would have attached.
///
/// `seen` collects every request body that reaches the module, so a test can
/// ask what the CLI actually put on the wire rather than only what came back.
async fn dns_app(store: &Store, seen: Capture) -> Router {
    let module = Beacon::default();
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: Leadership::always(),
    };
    Router::new()
        .nest("/dns", module.router(ctx))
        .layer(middleware::from_fn(add_caller))
        .layer(middleware::from_fn_with_state(seen, capture))
}

/// Bodies captured off the wire, newest last.
type Capture = Arc<Mutex<Vec<Value>>>;

/// Records the JSON body of every write, before the module sees it. Reading
/// the body here and handing the request on intact is the whole point: the
/// CLI's request is what is examined, not a value the test already has.
async fn capture(
    State(seen): State<Capture>,
    req: Request<Body>,
    next: middleware::Next,
) -> axum::response::Response {
    let (parts, body) = req.into_parts();
    let is_write = matches!(
        parts.method,
        axum::http::Method::POST | axum::http::Method::PUT
    );
    if is_write {
        match axum::body::to_bytes(body, 1 << 20).await {
            Ok(bytes) => {
                if let Ok(v) = serde_json::from_slice::<Value>(&bytes) {
                    seen.lock().unwrap().push(v);
                }
                let body = Body::from(bytes);
                return next.run(Request::from_parts(parts, body)).await;
            }
            // Not readable as a body at all: hand back what the module would
            // have been handed, which is nothing.
            Err(_) => return axum::response::Response::default(),
        }
    }
    next.run(Request::from_parts(parts, body)).await
}

async fn add_caller(mut req: Request<Body>, next: middleware::Next) -> axum::response::Response {
    req.extensions_mut().insert(caller());
    next.run(req).await
}

fn beacon(store: &Store) -> BeaconClient {
    BeaconClient::new(store.clone())
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

    async fn get(&self, path: &str) -> (StatusCode, Value) {
        self.send("GET", path, None).await
    }

    async fn post(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send("POST", path, Some(body)).await
    }

    async fn put(&self, path: &str, body: Value) -> (StatusCode, Value) {
        self.send("PUT", path, Some(body)).await
    }

    /// The zones as the store holds them, keyed by resource name.
    fn zones(&self) -> Vec<(String, Value)> {
        named(beacon(&self.store).zones().list(None).unwrap())
    }

    /// The record sets as the store holds them, keyed by resource name.
    fn records(&self) -> Vec<(String, Value)> {
        named(beacon(&self.store).recordsets().list(None).unwrap())
    }

    fn zone_rows(&self) -> Vec<Value> {
        self.zones().into_iter().map(|(_, z)| z).collect()
    }

    fn record_rows(&self) -> Vec<Value> {
        self.records().into_iter().map(|(_, r)| r).collect()
    }

    fn record_count(&self) -> usize {
        self.record_rows().len()
    }

    fn zone(&self, name: &str) -> Value {
        let mut names = Vec::new();
        for (n, z) in self.zones() {
            if n == name {
                return z;
            }
            names.push(n);
        }
        panic!("no zone named {name}; the store holds {names:?}")
    }

    fn record(&self, name: &str) -> Value {
        let mut names = Vec::new();
        for (n, r) in self.records() {
            if n == name {
                return r;
            }
            names.push(n);
        }
        panic!("no record set named {name}; the store holds {names:?}")
    }

    fn names(rows: &[(String, Value)]) -> Vec<String> {
        rows.iter().map(|(n, _)| n.clone()).collect()
    }

    /// Every body sent so far.
    fn bodies(&self) -> Vec<Value> {
        self.seen.lock().unwrap().clone()
    }
}

fn named<K>(items: Vec<K>) -> Vec<(String, Value)>
where
    K: Kind + DeserializeOwned,
{
    items
        .into_iter()
        .map(|k| {
            let name = k.meta().name.clone();
            (name, serde_json::to_value(k).unwrap())
        })
        .collect()
}

/// `zone-0199a5e0-…-00000001` -> `zone-00000001`, the CLI's own short form.
fn short(id: &str) -> String {
    let (prefix, uuid) = id.split_once('-').unwrap();
    let hex: String = uuid.chars().filter(|c| *c != '-').collect();
    let tail = hex.get(hex.len().saturating_sub(8)..).unwrap_or(&hex);
    format!("{prefix}-{tail}")
}

/// Stamp the status a reconciled zone would carry: the serial and phase the
/// table shows. The reconciler itself does not run in these tests — this is
/// the one write a test makes behind the module's back, and only to `status`,
/// which is the reconciler's field and nobody's input.
fn stamp_zone(s: &Srv, id: &str, serial: u32, records: u32) {
    let mut z = beacon(&s.store)
        .zones()
        .get(&Id::parse(kind::ZONE, id).unwrap())
        .unwrap();
    z.status.serial = serial;
    z.status.records = records;
    z.status.phase = Phase::Ready;
    beacon(&s.store).zones().update(z, None).unwrap();
}

/// A zone as an earlier `zone create` would have left it. Returns its id.
async fn seed_zone(s: &Srv, name: &str, apex: &str, email: &str) -> String {
    let (status, v) = s
        .post(
            "/dns/zones",
            json!({
                "name": name,
                "spec": {
                    "name": apex,
                    "kind": {"type": "primary"},
                    "ttl": 3600,
                    "email": email,
                },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

/// A record set as an earlier `record create` would have left it.
async fn seed_record(s: &Srv, name: &str, zone: &str, owner: &str, rtype: &str, rdata: &str) {
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": name,
                "spec": {
                    "zone": zone,
                    "name": owner,
                    "type": rtype,
                    "ttl": 300,
                    "records": [rdata],
                },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
}

/// The two zones every case starts from, each holding one set of its own:
/// `example.cloud.` with `web-a` (`web` → `10.0.0.4`) and `corp.cloud.` with
/// `api-a` (`api` → `10.9.0.7`). Returns `(example id, corp id)`.
async fn seed_pair(s: &Srv) -> (String, String) {
    let example = seed_zone(s, "example", "example.cloud.", "hostmaster@example.cloud").await;
    let corp = seed_zone(s, "corp", "corp.cloud.", "hostmaster@corp.cloud").await;
    seed_record(s, "web-a", &example, "web", "A", "10.0.0.4").await;
    seed_record(s, "api-a", &corp, "api", "A", "10.9.0.7").await;
    stamp_zone(s, &example, 2026111401, 1);
    stamp_zone(s, &corp, 2026111501, 1);
    (example, corp)
}

async fn serve() -> Srv {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let store = Store::in_memory().unwrap();
    let seen: Capture = Arc::new(Mutex::new(Vec::new()));
    let app = dns_app(&store, seen.clone()).await;
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
///
/// The renderer pads every column but the last and trims the line, and the CLI
///'s own columns can be empty — `also_notify` is absent for most zones, a
/// record set may carry no TTL — so a row cannot be split into cells and
/// counted: the only layout-independent read is the one that asks where the
/// header says a column starts. `cell` answers what is printed under a header,
/// which is the question a person reading the table asks.
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

    /// The offset a header starts at, in the header line.
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

    /// The row whose first column is `name`, as the raw line.
    fn line(&self, name: &str) -> &str {
        for l in &self.lines[1..] {
            if l.split_whitespace().next() == Some(name) {
                return l;
            }
        }
        panic!("no row for {name}: {:?}", self.lines)
    }

    /// What is printed under `header` in the row named `name`: the text up to
    /// the next column's offset, empty when the row has nothing there.
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

    /// The row count, header excluded.
    fn count(&self) -> usize {
        self.lines.len().saturating_sub(1)
    }
}

// ---- zones ---------------------------------------------------------------------

#[tokio::test]
async fn zone_list_and_show() {
    let s = serve().await;
    let (example, corp) = seed_pair(&s).await;

    let r = pi(&s, &["dns", "zone", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let t = Table::new(&r.out);
    assert_eq!(
        t.headers,
        ["NAME", "ID", "PHASE", "APEX", "TTL", "EMAIL", "SERIAL", "RECORDS", "AGE"]
    );
    // Both zones, by name — the row order is the CLI's, not the store's.
    for (name, id, apex, email, serial) in [
        (
            "corp",
            &corp,
            "corp.cloud.",
            "hostmaster@corp.cloud",
            "2026111501",
        ),
        (
            "example",
            &example,
            "example.cloud.",
            "hostmaster@example.cloud",
            "2026111401",
        ),
    ] {
        assert_eq!(t.cell(name, "ID"), short(id), "{name}");
        assert_eq!(t.cell(name, "PHASE"), "ready", "{name}");
        assert_eq!(t.cell(name, "APEX"), apex, "{name}");
        assert_eq!(t.cell(name, "TTL"), "3600", "{name}");
        assert_eq!(t.cell(name, "EMAIL"), email, "{name}");
        assert_eq!(t.cell(name, "SERIAL"), serial, "{name}");
        assert_eq!(t.cell(name, "RECORDS"), "1", "{name}");
        assert_eq!(t.cell(name, "AGE"), "0s", "{name}");
    }

    // Wide adds the zone kind, its nameservers and the full id.
    let r = pi(&s, &["dns", "zone", "list", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let wide = Table::new(&r.out);
    assert!(
        wide.headers.iter().any(|h| h == "KIND"),
        "{:?}",
        wide.headers
    );
    assert!(
        wide.headers.iter().any(|h| h == "NAMESERVERS"),
        "{:?}",
        wide.headers
    );
    assert!(r.out.contains(&example), "{}", r.out);
    // The kind is the tagged object's type. Nameservers are the module's
    // default — `ns1.<apex>` — and the module leaves a default field out of
    // the document entirely, so the column is a hole, not a guess the CLI
    // fills with something the server never sent.
    assert_eq!(wide.cell("example", "KIND"), "primary");
    assert_eq!(wide.cell("example", "NAMESERVERS"), "");
    // Ask for them and they are there.
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "ns1.example",
            "--nameserver",
            "ns1.ns1.example.",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let wide = Table::new(&pi(&s, &["dns", "zone", "list", "-o", "wide"]).await.out);
    assert_eq!(wide.cell("ns1.example", "NAMESERVERS"), "ns1.ns1.example.");

    // Show takes a name, an id or the apex.
    for key in ["example", &example, "example.cloud."] {
        let r = pi(&s, &["dns", "zone", "show", key]).await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
        assert!(r.out.contains("example.cloud."), "{}", r.out);
    }

    let r = pi(&s, &["dns", "zone", "show", "example", "-o", "json"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["id"], example);
    // The document carries the tagged kind, not a bare word.
    assert_eq!(v["spec"]["kind"], json!({"type": "primary"}));
    // A field at the module's own default is absent from the document: the
    // zone kept the default nameservers, and says nothing about them.
    assert!(v["spec"].get("nameservers").is_none(), "{v}");

    let r = pi(&s, &["dns", "zone", "show", "nope"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn zone_create_sends_a_primary_zone_spec() {
    let s = serve().await;
    seed_pair(&s).await;
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "lab.example",
            "--ttl",
            "600",
            "--email",
            "ops@lab.example",
            "--nameserver",
            "ns1.lab.example.",
            "--description",
            "the lab",
            "--label",
            "env=lab",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let created = s.zone("lab.example");
    // The id in the CLI's line is the id the module filed the zone under.
    assert!(
        r.out.starts_with(&format!(
            "zone \"lab.example\" created ({})",
            short(created["id"].as_str().unwrap())
        )),
        "{}",
        r.out
    );
    // The name defaults to the apex without its final dot; the apex is stored absolute.
    assert_eq!(created["spec"]["name"], "lab.example.");
    assert_eq!(created["spec"]["kind"], json!({"type": "primary"}));
    assert_eq!(created["spec"]["ttl"], 3600 / 6);
    assert_eq!(created["spec"]["email"], "ops@lab.example");
    assert_eq!(created["spec"]["nameservers"], json!(["ns1.lab.example."]));
    assert_eq!(created["description"], "the lab");
    assert_eq!(created["labels"]["env"], "lab");

    // A zone already covering the apex is refused, and the error is the server's.
    let r = pi(&s, &["dns", "zone", "create", "example.cloud."]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("already exists"), "{}", r.err);

    // An explicit name is kept as given.
    let r = pi(
        &s,
        &["dns", "zone", "create", "corp.example.", "--name", "corp2"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.starts_with("zone \"corp2\" created"), "{}", r.out);
    assert_eq!(s.zone("corp2")["spec"]["name"], "corp.example.");

    // No --email: the CLI sends no mailbox and the module's default answers,
    // which is what the SOA then carries.
    let r = pi(&s, &["dns", "zone", "create", "bare.example"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(s.zone("bare.example")["spec"]["email"], "hostmaster");

    // The default name is the apex with its one trailing dot dropped, not
    // everything from the first dot: `lab.example` keeps its middle dot, and
    // a two-label apex keeps both. `example.cloud.` becomes `example.cloud`.
    let r = pi(&s, &["dns", "zone", "create", "deep.sub.example"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        s.zone("deep.sub.example")["spec"]["name"],
        "deep.sub.example."
    );
    let r = pi(&s, &["dns", "zone", "create", "dotted.cloud."]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(s.zone("dotted.cloud")["spec"]["name"], "dotted.cloud.");
}

#[tokio::test]
async fn zone_create_takes_also_notify_addresses() {
    let s = serve().await;
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "pub.example",
            "--also-notify",
            "203.0.113.53",
            "--also-notify",
            "203.0.113.54:5353",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        s.zone("pub.example")["spec"]["also_notify"],
        json!(["203.0.113.53", "203.0.113.54:5353"])
    );

    // The addresses are the wide listing's NOTIFY column.
    let r = pi(&s, &["dns", "zone", "list", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let wide = Table::new(&r.out);
    assert!(
        wide.headers.iter().any(|h| h == "NOTIFY"),
        "{:?}",
        wide.headers
    );
    assert!(r.out.contains("203.0.113.53"), "{}", r.out);

    // A non-address is the module's refusal, phrased by the module.
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "bad.example",
            "--also-notify",
            "ns1.example.com",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("is not an ip address"), "{}", r.err);
}

#[tokio::test]
async fn zone_create_with_a_primary_makes_a_secondary() {
    let s = serve().await;
    seed_pair(&s).await;
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "copy.example",
            "--name",
            "copy",
            "--primary",
            "198.51.100.9",
            "--primary",
            "198.51.100.10:5353",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.starts_with("zone \"copy\" created"), "{}", r.out);
    let z = s.zone("copy");
    assert_eq!(
        z["spec"]["kind"],
        json!({"type": "secondary", "primaries": ["198.51.100.9", "198.51.100.10:5353"]})
    );
    // A secondary is not served until a transfer fills it, and until it has
    // been filled it stays Pending. (`status.message` is the reconciler's
    // field and the reconciler does not run here, so this says nothing about
    // it: see `the_status_column_shows_the_reconciler_s_work`.)
    assert_eq!(z["status"]["phase"], "pending");
    assert_eq!(z["status"]["serial"], 0);

    // The wide table names the primaries it pulls from.
    let r = pi(&s, &["dns", "zone", "list", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let wide = Table::new(&r.out);
    assert_eq!(wide.cell("copy", "KIND"), "secondary");
    assert_eq!(
        wide.cell("copy", "PRIMARY"),
        "198.51.100.9,198.51.100.10:5353"
    );
    // A primary's PRIMARY column stays an honest `primary`, and its NOTIFY — a
    // field the module leaves out when empty — is a hole read at its own index.
    assert_eq!(wide.cell("example", "PRIMARY"), "primary");
    assert_eq!(wide.cell("example", "NOTIFY"), "");
    assert_eq!(wide.cell("example", "KIND"), "primary");

    // Record sets cannot be added to a secondary: the module refuses.
    let r = pi(
        &s,
        &[
            "dns", "record", "create", "web", "A", "--zone", "copy", "--record", "10.0.0.1",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("record sets belong on primary zones"),
        "{}",
        r.err
    );

    // --primary and --also-notify are a contradiction, refused before anything
    // goes out: a secondary is not a primary's notifier.
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "both.example",
            "--primary",
            "198.51.100.9",
            "--also-notify",
            "203.0.113.53",
        ],
    )
    .await;
    assert_eq!(r.code, 2);
    assert!(r.err.contains("--also-notify"), "{}", r.err);
}

#[tokio::test]
async fn zone_delete_needs_yes_or_a_terminal_and_takes_an_id() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;

    // No terminal, no --yes: refused before anything is sent.
    let r = pi(&s, &["dns", "zone", "delete", "example"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert_eq!(s.zone_rows().len(), 2);

    // An id is resolved so the message names the zone.
    let r = pi(&s, &["dns", "zone", "delete", &example, "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "zone \"example\" deleted\n");
    assert_eq!(s.zone_rows().len(), 1);
    // Only the deleted zone's record set went with it.
    assert_eq!(s.record_rows().len(), 1);

    let r = pi(&s, &["dns", "zone", "delete", "example", "--yes"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

/// The scope of `zone delete`, stated where someone will read it before
/// typing the command. The module has no refusal here: a zone delete takes
/// every record set in the zone with it, and there is nothing to confirm it
/// away — the CLI's only protection is the `--yes` the command demands.
#[tokio::test]
async fn a_zone_delete_cascades_and_the_module_does_not_refuse() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    // The zone is not empty: it holds a set, and more than one would be the
    // same. The server still answers 204.
    assert_eq!(
        beacon(&s.store)
            .recordsets_in_zone(&Id::parse(kind::ZONE, &example).unwrap())
            .unwrap()
            .len(),
        1
    );
    let r = pi(&s, &["dns", "zone", "delete", "example", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        !r.err.contains("refus") && !r.err.contains("not empty"),
        "there is no refusal to surface: {}",
        r.err
    );
    assert_eq!(s.record_count(), 1);
    assert!(
        Srv::names(&s.records()).contains(&"api-a".to_owned()),
        "only the deleted zone's sets went"
    );
    // The cascade is the module's, done through the same route a plain API
    // caller would use — not something the CLI papers over.
    let corp = s.zone("corp")["id"].as_str().unwrap().to_owned();
    seed_record(&s, "x-a", &corp, "x", "A", "10.9.0.99").await;
    let (status, _) = s.get(&format!("/dns/zones/{corp}")).await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = s.send("DELETE", &format!("/dns/zones/{corp}"), None).await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(s.zone_rows().is_empty());
    assert!(s.record_rows().is_empty());
}

/// The same cascade seen from the CLI, which is what a person actually does:
/// a zone and everything inside it go, and the surviving zone's sets stay.
#[tokio::test]
async fn deleting_a_zone_takes_its_records_with_it() {
    let s = serve().await;
    seed_pair(&s).await;
    let r = pi(&s, &["dns", "zone", "delete", "example", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(s.zone_rows().len(), 1);
    // The zone's own set went; the other zone's stayed.
    assert_eq!(s.record_rows().len(), 1);
    assert!(Srv::names(&s.records()).contains(&"api-a".to_owned()));
    let r = pi(&s, &["dns", "record", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(!r.out.contains("web-a"), "{}", r.out);
    assert!(r.out.contains("api-a"), "{}", r.out);
    let r = pi(&s, &["dns", "zone", "delete", "corp", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(s.zone_rows().is_empty());
    assert!(s.record_rows().is_empty());
    let r = pi(&s, &["dns", "record", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(!r.out.contains("api-a"), "{}", r.out);
}

// ---- record sets -----------------------------------------------------------------

#[tokio::test]
async fn record_list_filters_by_zone_and_prints_the_records() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;

    let r = pi(&s, &["dns", "record", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let t = Table::new(&r.out);
    assert_eq!(
        t.headers,
        ["NAME", "ID", "OWNER", "TYPE", "TTL", "RECORDS", "AGE"]
    );
    for (name, owner, rdata) in [
        ("api-a", "api.corp.cloud.", "10.9.0.7"),
        ("web-a", "web.example.cloud.", "10.0.0.4"),
    ] {
        assert_eq!(
            t.cell(name, "ID"),
            short(s.record(name)["id"].as_str().unwrap()),
            "{name}"
        );
        assert_eq!(t.cell(name, "OWNER"), owner, "{name}");
        assert_eq!(t.cell(name, "TYPE"), "A", "{name}");
        assert_eq!(t.cell(name, "TTL"), "300", "{name}");
        assert_eq!(t.cell(name, "RECORDS"), rdata, "{name}");
        assert_eq!(t.cell(name, "AGE"), "0s", "{name}");
    }

    // --zone takes a name, an apex or an id, and only that zone's sets come back.
    for key in ["example", "example.cloud.", &example] {
        let r = pi(&s, &["dns", "record", "list", "--zone", key]).await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
        assert_eq!(Table::new(&r.out).count(), 1, "{key}: {}", r.out);
        assert!(r.out.contains("web-a"), "{key}: {}", r.out);
        assert!(!r.out.contains("api-a"), "{key}: {}", r.out);
    }

    // The filter is the server's, not the CLI's: asked of the module directly,
    // the same page comes back narrowed, so `--zone` cannot be silently
    // ignoring what it names.
    let (status, page) = s.get(&format!("/dns/recordsets?zone={example}")).await;
    assert_eq!(status, StatusCode::OK, "{page}");
    assert_eq!(page["items"].as_array().unwrap().len(), 1, "{page}");
    assert_eq!(page["items"][0]["name"], "web-a");

    // A zone with nothing in it lists nothing rather than failing.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "mail",
            "A",
            "--zone",
            "example",
            "--record",
            "10.0.0.11",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let r = pi(&s, &["dns", "record", "list", "-o", "name"]).await;
    // Unsorted formats pass the page through as the server sent it, so a
    // script sees the same stream `pi api` would give it.
    let mut lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", r.out);
    lines.sort_unstable();
    assert_eq!(lines, ["api-a", "mail-a", "web-a"]);

    let r = pi(&s, &["dns", "record", "list", "--zone", "ghost"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("zone 'ghost' not found"), "{}", r.err);
}

#[tokio::test]
async fn a_set_without_a_ttl_is_listed_with_its_zone_s() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "no-ttl",
                "spec": {"zone": example, "name": "plain", "type": "A", "records": ["10.0.0.30"]},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    // The server truly leaves the field out; the CLI is what fills the hole.
    assert!(v["spec"].get("ttl").is_none(), "{v}");

    let r = pi(&s, &["dns", "record", "list", "--zone", "example"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let t = Table::new(&r.out);
    assert_eq!(t.cell("no-ttl", "TTL"), "3600");
    assert_eq!(t.cell("no-ttl", "OWNER"), "plain.example.cloud.");
    // `show` reads the same fallback: one set must not read one way in the
    // list and another on its own.
    let r = pi(&s, &["dns", "record", "show", "no-ttl"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let shown = Table::new(&r.out);
    assert_eq!(shown.cell("no-ttl", "TTL"), "3600");

    // The zone's TTL only fills the table. The document a script reads is the
    // row the server sent, TTL still absent.
    let r = pi(&s, &["dns", "record", "show", "no-ttl", "-o", "json"]).await;
    let shown: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(shown["spec"], v["spec"]);
    let r = pi(&s, &["dns", "record", "list", "-o", "json"]).await;
    let listed: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    let row = listed.iter().find(|it| it["name"] == "no-ttl").unwrap();
    assert!(row["spec"].get("ttl").is_none(), "{row}");
}

#[tokio::test]
async fn record_show_takes_a_name_or_an_id() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    let rec = s.record("web-a")["id"].as_str().unwrap().to_owned();
    for key in ["web-a", &rec] {
        let r = pi(&s, &["dns", "record", "show", key]).await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
        assert!(r.out.contains("web.example.cloud."), "{}", r.out);
        assert!(r.out.contains("10.0.0.4"), "{}", r.out);
    }
    let r = pi(&s, &["dns", "record", "show", "web-a", "-o", "yaml"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("type: A"), "{}", r.out);

    let r = pi(&s, &["dns", "record", "show", "nothing"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);

    // --zone scopes the lookup: the name is found among that zone's sets, by
    // zone name, apex or id alike.
    for key in ["example", "example.cloud.", &example] {
        let r = pi(&s, &["dns", "record", "show", "web-a", "--zone", key]).await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
        assert!(r.out.contains("web.example.cloud."), "{key}: {}", r.out);
        assert!(!r.err.contains("warning"), "{key}: {}", r.err);
    }
    // The id works under --zone too.
    let r = pi(&s, &["dns", "record", "show", &rec, "--zone", "example"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("10.0.0.4"), "{}", r.out);
    // A name the zone does not hold is a usage error naming both.
    let r = pi(&s, &["dns", "record", "show", "api-a", "--zone", "example"]).await;
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("record set 'api-a' not found in zone 'example'"),
        "{}",
        r.err
    );
    // An unknown zone is the same usage error list and create give.
    let r = pi(&s, &["dns", "record", "show", "web-a", "--zone", "ghost"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("zone 'ghost' not found"), "{}", r.err);
}

#[tokio::test]
async fn record_show_zone_looks_the_name_up_inside_one_zone() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    // A set filed under `api-a` — the name corp's own set uses — in the
    // example zone, which is exactly the collision --zone exists to settle.
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "api-a",
                "spec": {"zone": example, "name": "api", "type": "A", "records": ["10.0.0.21"]},
            }),
        )
        .await;
    // One name cannot hold two rows: the store's name index refuses the
    // second, the way it would refuse a second zone called `example`. That is
    // also why a default record name carries its zone's apex — see
    // `a_default_name_separates_two_zones_holding_the_same_host`.
    assert_eq!(status, StatusCode::CONFLICT, "{v}");

    // -o name answers with just the name it found.
    let r = pi(
        &s,
        &[
            "dns", "record", "show", "api-a", "--zone", "example", "-o", "name",
        ],
    )
    .await;
    // The name exists in the store but not in this zone: the zone-scoped walk
    // is the one that answers, and it answers with the usage error.
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("record set 'api-a' not found in zone 'example'"),
        "{}",
        r.err
    );
    // The corp set by its own name, under its own zone.
    let r = pi(
        &s,
        &[
            "dns", "record", "show", "api-a", "--zone", "corp", "-o", "name",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "api-a\n");
    let r = pi(
        &s,
        &[
            "dns", "record", "show", "api-a", "--zone", "corp", "-o", "wide",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("api.corp.cloud."), "{}", r.out);
    // A name the zone holds nothing for is the usage error naming both.
    let r = pi(&s, &["dns", "record", "show", "db-a", "--zone", "corp"]).await;
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("record set 'db-a' not found in zone 'corp'"),
        "{}",
        r.err
    );
}

#[tokio::test]
async fn record_create_resolves_the_zone_and_names_the_set() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "db",
            "a",
            "--zone",
            "example",
            "--ttl",
            "60",
            "--record",
            "10.0.0.9",
            "--record",
            "10.0.0.10",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    // The default name is the one the module files an automatic set under.
    let created = s.record("db-a");
    assert!(
        r.out.starts_with(&format!(
            "record \"db-a\" created ({})",
            short(created["id"].as_str().unwrap())
        )),
        "{}",
        r.out
    );
    assert_eq!(created["spec"]["zone"], example);
    assert_eq!(created["spec"]["name"], "db.example.cloud.");
    assert_eq!(created["spec"]["type"], "A");
    assert_eq!(created["spec"]["ttl"], 60);
    assert_eq!(created["spec"]["records"], json!(["10.0.0.9", "10.0.0.10"]));

    // An explicit name wins over the default.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "@",
            "TXT",
            "--name",
            "primary-db",
            "--zone",
            "example.cloud.",
            "--record",
            "v=spf1 -all",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.out.starts_with("record \"primary-db\" created"),
        "{}",
        r.out
    );
    let apex_set = s.record("primary-db");
    assert_eq!(apex_set["spec"]["name"], "example.cloud.");
    // The module stores TXT quoted, whatever the caller wrote.
    assert_eq!(apex_set["spec"]["records"], json!(["\"v=spf1 -all\""]));

    // Without --ttl the field is left out so the zone's own TTL applies.
    assert!(apex_set["spec"].get("ttl").is_none());

    let r = pi(
        &s,
        &[
            "dns", "record", "create", "web", "A", "--zone", "example", "--record", "10.0.0.6",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("already has a A set"), "{}", r.err);
}

// ---- the name a record set is filed under -----------------------------------------

/// A zone-scoped lookup answers with a row the zone holds, whether that row is
/// filed under the short default name or the one carrying the zone after it.
#[tokio::test]
async fn a_zone_lookup_finds_either_form_of_the_default_name() {
    let s = serve().await;
    let (example, corp) = seed_pair(&s).await;
    // One row filed short, one filed long, in the same zone, through the API
    // directly — the shapes a project already holds before `pi` sees them.
    for (name, zone, owner, ip) in [
        ("r1-a", &example, "r1.example.cloud.", "10.0.0.70"),
        (
            "r2-a.example.cloud",
            &example,
            "r2.example.cloud.",
            "10.0.0.71",
        ),
    ] {
        let (status, v) = s
            .post(
                "/dns/recordsets",
                json!({
                    "name": name,
                    "spec": {"zone": zone, "name": owner, "type": "A", "records": [ip]},
                }),
            )
            .await;
        assert_eq!(status, StatusCode::CREATED, "{v}");
    }

    for (filed, short) in [("r1-a", "r1-a"), ("r2-a.example.cloud", "r2-a")] {
        for key in [filed, short] {
            for zone in ["example", "example.cloud.", &example] {
                let r = pi(
                    &s,
                    &["dns", "record", "show", key, "--zone", zone, "-o", "name"],
                )
                .await;
                assert_eq!(r.code, 0, "{key}@{zone}: {}", r.err);
                assert_eq!(r.out, format!("{filed}\n"), "{key}@{zone}");
                assert!(!r.err.contains("warning"), "{key}@{zone}: {}", r.err);
            }
        }
    }

    // Delete reaches the same rows the same way, and reports the filed name.
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", "r2-a", "--zone", "example", "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "record \"r2-a.example.cloud\" deleted\n");
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", "r1-a", "--zone", &example, "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "record \"r1-a\" deleted\n");
    assert_eq!(Srv::names(&s.records()), ["web-a", "api-a"]);
    let _ = &corp;
}

/// Without `--zone` a record set is still the store's own row: it is read by
/// the name it is filed under, and a short form that is not that name is not
/// found anywhere.
#[tokio::test]
async fn outside_a_zone_only_the_filed_name_resolves() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "r-a.example.cloud",
                "spec": {"zone": example, "name": "r.example.cloud.", "type": "A", "records": ["10.0.0.72"]},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");

    let r = pi(
        &s,
        &["dns", "record", "show", "r-a.example.cloud", "-o", "name"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "r-a.example.cloud\n");
    let r = pi(&s, &["dns", "record", "show", "r-a"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
    let r = pi(&s, &["dns", "record", "delete", "r-a", "--yes"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
    assert_eq!(s.record_count(), 3);
}

/// A resource name is unique in a project, not in a zone, so two zones holding
/// the same host both want `www-a` and the second create is refused by a name
/// belonging to the other zone's set. The default keeps them apart: a set takes
/// the short name when it is free, and the zone's apex after it when it is not.
#[tokio::test]
async fn a_default_name_separates_two_zones_holding_the_same_host() {
    let s = serve().await;
    let (example, corp) = seed_pair(&s).await;

    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "www",
            "A",
            "--zone",
            "example",
            "--record",
            "10.0.0.60",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    // The first one keeps the short name the module would have given it.
    assert!(r.out.starts_with("record \"www-a\" created"), "{}", r.out);
    let first = s.record("www-a");
    assert_eq!(first["spec"]["zone"], example);

    // The second is not refused by the first's name: it takes the zone with it.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "www",
            "A",
            "--zone",
            "corp",
            "--record",
            "10.9.0.60",
        ],
    )
    .await;
    assert_eq!(
        r.code, 0,
        "the second zone's set must not be refused: {}",
        r.err
    );
    assert!(
        r.out.starts_with("record \"www-a.corp.cloud\" created"),
        "{}",
        r.out
    );
    let second = s.record("www-a.corp.cloud");
    assert_eq!(second["spec"]["zone"], corp);
    assert_eq!(second["spec"]["records"], json!(["10.9.0.60"]));
    assert_eq!(first["spec"]["records"], json!(["10.0.0.60"]));

    // Both are readable, each by the name it was filed under; the short form
    // still means the one that holds it, inside its own zone.
    let r = pi(&s, &["dns", "record", "show", "www-a", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).unwrap()["id"],
        first["id"]
    );
    let r = pi(
        &s,
        &["dns", "record", "show", "www-a.corp.cloud", "-o", "json"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).unwrap()["id"],
        second["id"]
    );
    let r = pi(
        &s,
        &[
            "dns", "record", "show", "www-a", "--zone", "corp", "-o", "json",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        serde_json::from_str::<Value>(&r.out).unwrap()["id"],
        second["id"]
    );

    // A name typed by the caller is sent as typed, once: a clash is reported,
    // not worked around behind their back.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "api",
            "A",
            "--name",
            "api-a",
            "--zone",
            "example",
            "--record",
            "10.0.0.61",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("already exists"), "{}", r.err);

    // And the zone's listing is unaffected by which name each row carries.
    let r = pi(
        &s,
        &["dns", "record", "list", "--zone", "corp", "-o", "name"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let mut lines: Vec<&str> = r.out.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["api-a", "www-a.corp.cloud"]);
}

/// A set already filed under the long form is still reached by its short one
/// when the zone is named, which is the point of `--zone` on `show` and delete.
#[tokio::test]
async fn a_zone_finding_reads_the_short_name_of_a_set_filed_long() {
    let s = serve().await;
    let (_, corp) = seed_pair(&s).await;
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "www",
            "A",
            "--zone",
            "example",
            "--record",
            "10.0.0.60",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "www",
            "A",
            "--zone",
            "corp",
            "--record",
            "10.9.0.60",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);

    // Short name, long name and id all reach the corp set inside its zone.
    let id = s.record("www-a.corp.cloud")["id"]
        .as_str()
        .unwrap()
        .to_owned();
    for key in ["www-a", "www-a.corp.cloud", &id] {
        let r = pi(
            &s,
            &["dns", "record", "show", key, "--zone", "corp", "-o", "json"],
        )
        .await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
        assert_eq!(
            serde_json::from_str::<Value>(&r.out).unwrap()["id"],
            id,
            "{key}"
        );
        assert!(!r.err.contains("warning"), "{key}: {}", r.err);
    }
    // Under the wrong zone the same short name finds the other set, and a name
    // that zone holds nothing for is still the usage error naming both.
    let r = pi(
        &s,
        &[
            "dns", "record", "show", "www-a", "--zone", "example", "-o", "name",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "www-a\n");
    let r = pi(&s, &["dns", "record", "show", "db-a", "--zone", "corp"]).await;
    assert_eq!(r.code, 1);
    assert!(
        r.err.contains("record set 'db-a' not found in zone 'corp'"),
        "{}",
        r.err
    );
    // Outside a zone the filed name is the only name that resolves.
    let r = pi(&s, &["dns", "record", "show", "www-a", "-o", "name"]).await;
    assert_eq!(r.out, "www-a\n");
    let r = pi(
        &s,
        &["dns", "record", "delete", "www-a.corp.cloud", "--yes"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(Srv::names(&s.records()), ["web-a", "api-a", "www-a"]);
    let _ = &corp;
}

#[tokio::test]
async fn a_bad_record_is_refused_and_nothing_is_left_behind() {
    let s = serve().await;
    seed_pair(&s).await;

    // A type the module does not know is caught locally, with the list.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "web",
            "CNAMEE",
            "--zone",
            "example",
            "--record",
            "web.example.cloud.",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("unknown record type"), "{}", r.err);
    assert!(
        r.err.contains("A, AAAA, CNAME, MX, TXT, SRV, NS, PTR"),
        "{}",
        r.err
    );

    // A zone the caller does not have is a usage error, phrased by the CLI.
    let r = pi(
        &s,
        &[
            "dns", "record", "create", "web", "A", "--zone", "nowhere", "--record", "10.0.0.6",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("zone 'nowhere' not found"), "{}", r.err);

    // A record set with no data is refused by clap, not the server.
    let r = pi(
        &s,
        &["dns", "record", "create", "web", "A", "--zone", "example"],
    )
    .await;
    assert_eq!(r.code, 2);
    assert!(r.err.contains("--record"), "{}", r.err);

    // RDATA that is not right for the type comes back as the server's error.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "web",
            "A",
            "--zone",
            "example",
            "--record",
            "not-an-address",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not valid for this type"), "{}", r.err);

    // Bad RDATA is refused before the call goes out, so a typo cannot reach
    // the server at all — the exact shape a person mistypes: a value in the
    // record's place.
    let before = s.record_count();
    for bad in ["not-an-ip", "10.0.0", "300.1.1.1", "10.0.0.4,10.0.0.5"] {
        let r = pi(
            &s,
            &[
                "dns", "record", "create", "bad", "A", "--zone", "example", "--record", bad,
            ],
        )
        .await;
        assert_eq!(r.code, 1, "{bad}: {}", r.out);
        assert!(
            r.err.contains("A record 'not valid for this type'")
                || (r.err.contains("A record") && r.err.contains("not valid")),
            "{bad}: {}",
            r.err
        );
    }
    // An AAAA asked for in dotted-quad, and a CNAME without its final dot.
    let r = pi(
        &s,
        &[
            "dns", "record", "create", "bad", "AAAA", "--zone", "example", "--record", "10.0.0.4",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("AAAA record"), "{}", r.err);
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "bad",
            "CNAME",
            "--zone",
            "example",
            "--record",
            "web.example.cloud",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("CNAME record"), "{}", r.err);
    assert_eq!(s.record_count(), before, "nothing was sent");

    // The shapes that are right do get sent, and go out as given.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "spf",
            "TXT",
            "--zone",
            "example",
            "--record",
            "\"v=spf1 -all\"",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(s.record_count(), before + 1);
    // MX and SRV want their own shapes; a bare name is refused locally.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "bad",
            "MX",
            "--zone",
            "example",
            "--record",
            "mail.example.cloud",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("MX record"), "{}", r.err);
    assert_eq!(s.record_count(), before + 1, "a refusal sends nothing");
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "ok",
            "MX",
            "--zone",
            "example",
            "--record",
            "10 mail.example.cloud.",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(s.record_count(), before + 2);

    // Nothing half-made by the refused ones: the seeded sets, plus the two
    // the good calls above created.
    let mut names = Srv::names(&s.records());
    names.sort();
    assert_eq!(
        names,
        vec![
            "api-a".to_owned(),
            "ok-mx".to_owned(),
            "spf-txt".to_owned(),
            "web-a".to_owned()
        ]
    );
}

#[tokio::test]
async fn record_delete_needs_yes_and_takes_a_name_or_id() {
    let s = serve().await;
    seed_pair(&s).await;
    let rec = s.record("web-a")["id"].as_str().unwrap().to_owned();

    let r = pi(&s, &["dns", "record", "delete", "web-a"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert_eq!(s.record_count(), 2);

    let r = pi(&s, &["dns", "record", "delete", &rec, "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "record \"web-a\" deleted\n");
    assert_eq!(s.record_count(), 1);

    let r = pi(&s, &["dns", "record", "delete", "web-a", "--yes"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

/// `--zone` on delete means what it means on `show`: the set is looked up
/// inside that zone, so a name two zones share only takes one of them out.
#[tokio::test]
async fn record_delete_takes_a_zone_and_a_short_name() {
    let s = serve().await;
    let (example, corp) = seed_pair(&s).await;
    for zone in ["example", "corp"] {
        let r = pi(
            &s,
            &[
                "dns",
                "record",
                "create",
                "www",
                "A",
                "--zone",
                zone,
                "--record",
                "10.0.0.60",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{zone}: {}", r.err);
    }
    assert_eq!(s.record_count(), 4);
    let corp_set = s.record("www-a.corp.cloud")["id"]
        .as_str()
        .unwrap()
        .to_owned();

    // Refused first, with nothing removed, as a delete without a zone is.
    let r = pi(&s, &["dns", "record", "delete", "www-a", "--zone", "corp"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert_eq!(s.record_count(), 4);

    // The short form, scoped: only the named zone's set goes, and it is
    // reported by the name it was filed under.
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", "www-a", "--zone", "corp", "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "record \"www-a.corp.cloud\" deleted\n");
    assert_eq!(s.record_count(), 3);
    assert!(s.record("www-a")["id"].is_string());

    // A zone holding nothing of that name refuses rather than reaching into
    // another zone for it, and an unknown zone is the usual usage error.
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", "www-a", "--zone", "corp", "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(
        r.err
            .contains("record set 'www-a' not found in zone 'corp'"),
        "{}",
        r.err
    );
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", "www-a", "--zone", "ghost", "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("zone 'ghost' not found"), "{}", r.err);

    // Zone as apex or id, set as filed name or id: all reach the same row.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "delete",
            "www-a",
            "--zone",
            "corp.cloud.",
            "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", "www-a", "--zone", &example, "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "record \"www-a\" deleted\n");
    let r = pi(
        &s,
        &[
            "dns", "record", "delete", &corp_set, "--zone", "corp", "--yes",
        ],
    )
    .await;
    assert_eq!(r.code, 1);
    assert_eq!(Srv::names(&s.records()), ["web-a", "api-a"]);
    let _ = &corp;
}

/// Deleting by a bare name still resolves through the store's name index, and
/// a name two zones both want is the one row the name index can hold.
#[tokio::test]
async fn record_delete_without_a_zone_takes_the_stored_name() {
    let s = serve().await;
    let (example, corp) = seed_pair(&s).await;
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "www-a",
                "spec": {"zone": example, "name": "www", "type": "A", "records": ["10.0.0.61"]},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "www-a",
                "spec": {"zone": corp, "name": "www", "type": "A", "records": ["10.9.0.61"]},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CONFLICT, "{v}");

    let r = pi(&s, &["dns", "record", "delete", "www-a", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "record \"www-a\" deleted\n");
    assert_eq!(s.record_count(), 2);
}

#[tokio::test]
async fn json_and_name_output_come_back_as_the_server_sent_them() {
    let s = serve().await;
    seed_pair(&s).await;

    let r = pi(&s, &["dns", "record", "list", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(items.len(), 2);
    let mut names: Vec<&str> = items
        .iter()
        .map(|it| it["name"].as_str().unwrap())
        .collect();
    names.sort_unstable();
    assert_eq!(names, ["api-a", "web-a"]);
    let web = items
        .iter()
        .find(|it| it["name"] == "web-a")
        .expect("web-a in the page");
    assert_eq!(web["spec"]["name"], "web.example.cloud.");

    let r = pi(&s, &["dns", "record", "list", "-o", "yaml"]).await;
    let mut items: Vec<Value> = serde_yaml_ng::from_str(&r.out).unwrap();
    items.sort_by_key(|it| it["name"].as_str().unwrap_or_default().to_owned());
    assert_eq!(items[0]["name"], "api-a");
    assert_eq!(items[1]["name"], "web-a");

    let r = pi(&s, &["dns", "zone", "list", "-o", "name"]).await;
    let mut lines: Vec<&str> = r.out.lines().collect();
    lines.sort_unstable();
    assert_eq!(lines, ["corp", "example"]);

    // A create in json mode prints the whole new record, not a sentence.
    let r = pi(
        &s,
        &[
            "dns",
            "record",
            "create",
            "mail",
            "A",
            "--zone",
            "example",
            "--record",
            "10.0.0.11",
            "-o",
            "json",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["name"], "mail-a");
    assert_eq!(v["spec"]["records"], json!(["10.0.0.11"]));
}

// ---- the shapes the wire actually carries ------------------------------------------

/// The CLI may only send what the module's own types can read: a zone kind is
/// an internally tagged object, a record set's type travels under `type`, and
/// a field the CLI leaves out must be absent — never null, which the module's
/// typed fields refuse outright.
#[tokio::test]
async fn every_body_the_cli_sends_deserializes() {
    let s = serve().await;
    seed_pair(&s).await;

    for args in [
        vec!["dns", "zone", "create", "one.example"],
        vec![
            "dns",
            "zone",
            "create",
            "two.example",
            "--ttl",
            "120",
            "--email",
            "ops@two.example",
            "--nameserver",
            "ns1.two.example.",
            "--nameserver",
            "ns2.two.example.",
            "--also-notify",
            "203.0.113.9",
            "--description",
            "d",
            "--label",
            "a=b",
        ],
        vec![
            "dns",
            "zone",
            "create",
            "three.example",
            "--primary",
            "198.51.100.9",
        ],
    ] {
        let r = pi(&s, &args).await;
        assert_eq!(r.code, 0, "{args:?}: {}", r.err);
    }
    assert_eq!(
        s.zone("two.example")["spec"]["nameservers"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(s.zone("three.example")["spec"]["kind"]["type"], "secondary");

    for (i, args) in [
        vec![
            "dns", "record", "create", "w1", "A", "--zone", "example", "--record", "10.0.1.1",
        ],
        vec![
            "dns",
            "record",
            "create",
            "w2",
            "AAAA",
            "--zone",
            "example",
            "--ttl",
            "30",
            "--record",
            "2001:db8::1",
        ],
        vec![
            "dns", "record", "create", "w3", "TXT", "--zone", "example", "--record", "hello",
        ],
        vec![
            "dns",
            "record",
            "create",
            "w4",
            "SRV",
            "--zone",
            "example",
            "--record",
            "10 5 8443 svc.example.cloud.",
        ],
        vec![
            "dns",
            "record",
            "create",
            "w5",
            "NS",
            "--zone",
            "example",
            "--record",
            "ns1.example.cloud.",
        ],
    ]
    .into_iter()
    .enumerate()
    {
        let r = pi(&s, &args).await;
        assert_eq!(r.code, 0, "{i} {args:?}: {}", r.err);
    }

    // The rows on the way back carry the keys the CLI reads. A field the
    // module left out is absent, never null — the CLI must read it as missing
    // rather than as a value.
    let r = pi(&s, &["dns", "zone", "list", "-o", "json"]).await;
    let zones: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert!(zones.len() >= 5);
    for z in &zones {
        for key in ["name", "kind", "ttl", "email"] {
            assert!(!z["spec"][key].is_null(), "{key} missing from {z}");
        }
        assert!(z["spec"]["kind"].is_object(), "kind is not tagged: {z}");
        for key in ["phase", "serial", "records"] {
            assert!(!z["status"][key].is_null(), "{key} missing from {z}");
        }
    }
    let r = pi(&s, &["dns", "record", "list", "-o", "json"]).await;
    let sets: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert!(sets.len() >= 5);
    for rs in &sets {
        for key in ["zone", "name", "type", "records"] {
            assert!(!rs["spec"][key].is_null(), "{key} missing from {rs}");
        }
        assert!(rs["spec"]["records"].is_array(), "{rs}");
    }

    // The bodies as the module received them. This is the shape the CLI is
    // blamed for getting wrong: an option the caller did not give must be
    // absent from the request, never present and null, and the zone kind must
    // be the tagged object.
    let bodies = s.bodies();
    let zone_bodies: Vec<&Value> = bodies
        .iter()
        .filter(|b| b["spec"]["name"].as_str().is_some_and(|n| n.ends_with('.')))
        .collect();
    assert!(!zone_bodies.is_empty());
    for b in &zone_bodies {
        assert!(
            b["spec"]["kind"].is_object(),
            "kind must be tagged, not a bare word: {b}"
        );
        assert!(
            matches!(
                b["spec"]["kind"]["type"].as_str(),
                Some("primary") | Some("secondary")
            ),
            "the tag names a kind the module knows: {b}"
        );
        for key in ["email", "nameservers", "also_notify"] {
            assert!(
                b["spec"].get(key) != Some(&Value::Null),
                "{key} was sent as null, which the module cannot read: {b}"
            );
        }
        // Whatever is sent must be readable by the module's own spec type:
        // a nested object needs its tag, a list needs strings.
        if b["spec"].get("nameservers").is_some() {
            assert!(b["spec"]["nameservers"].is_array(), "{b}");
        }
        if b["spec"].get("also_notify").is_some() {
            assert!(b["spec"]["also_notify"].is_array(), "{b}");
        }
    }
    // The secondary's body carries its primaries inside the tag.
    let sec = bodies
        .iter()
        .find(|b| b["spec"]["kind"]["type"] == "secondary")
        .expect("a secondary was created");
    assert_eq!(sec["spec"]["kind"]["primaries"], json!(["198.51.100.9"]));
    // A record set's type travels under `type`, and no --ttl means no field.
    let set_bodies: Vec<&Value> = bodies
        .iter()
        .filter(|b| b["spec"]["records"].is_array())
        .collect();
    assert!(!set_bodies.is_empty());
    for b in &set_bodies {
        assert!(
            b["spec"].get("kind").is_none(),
            "the field is `type` on the wire: {b}"
        );
        assert!(b["spec"]["type"].is_string(), "{b}");
        assert_ne!(b["spec"]["type"], Value::Null, "type missing: {b}");
        assert!(
            b["spec"].get("ttl") != Some(&Value::Null),
            "ttl sent as null: {b}"
        );
    }
    assert!(
        set_bodies.iter().any(|b| b["spec"].get("ttl").is_none()),
        "a set created without --ttl should send no ttl at all"
    );
    assert!(
        set_bodies
            .iter()
            .any(|b| b["spec"]["ttl"].as_u64() == Some(30)),
        "a set created with --ttl should send the number"
    );

    // A row written the way another module writes one — no `ttl`, no labels —
    // still renders in every output the CLI offers.
    let corp = s.zone("corp")["id"].as_str().unwrap().to_owned();
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "auto-a",
                "spec": {"zone": corp, "name": "node1", "type": "A", "records": ["10.9.0.9"]},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    for fmt in ["table", "wide", "json", "yaml", "name"] {
        let r = pi(&s, &["-o", fmt, "dns", "record", "list", "--zone", "corp"]).await;
        assert_eq!(r.code, 0, "{fmt}: {}", r.err);
        assert!(r.out.contains("auto-a"), "{fmt}: {}", r.out);
    }
    let r = pi(&s, &["dns", "record", "show", "auto-a", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
}

/// A resource whose stored name needs quoting on a path — a name the CLI can
/// build but the server's path matcher cannot read as one token — must not be
/// answered as missing.
#[tokio::test]
async fn every_listed_resource_can_be_shown_by_its_own_name() {
    let s = serve().await;
    seed_pair(&s).await;
    // A secondary named by its apex: the default name keeps the dot the apex
    // carries, which is the shape most likely to be mistaken for one.
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "copy.example",
            "--primary",
            "198.51.100.9",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let listed = pi(&s, &["dns", "zone", "list", "-o", "name"]).await;
    assert_eq!(listed.code, 0, "{}", listed.err);
    for name in listed.out.lines() {
        let r = pi(&s, &["dns", "zone", "show", name]).await;
        assert_eq!(r.code, 0, "{name}: {}", r.err);
    }
    // And the same name the listing shows is the name delete reads back.
    let r = pi(&s, &["dns", "zone", "delete", "copy.example", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, "zone \"copy.example\" deleted\n");
}

/// The column set is a fixed shape: a row missing a field renders as a hole,
/// never as a shifted row or a failed command.
#[tokio::test]
async fn absent_fields_render_as_holes_not_shifted_columns() {
    let s = serve().await;
    let (example, corp) = seed_pair(&s).await;
    let r = pi(
        &s,
        &[
            "dns",
            "zone",
            "create",
            "sec.cloud",
            "--name",
            "sec",
            "--primary",
            "198.51.100.9",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);

    let narrow = Table::new(&pi(&s, &["dns", "zone", "list"]).await.out);
    let wide = Table::new(&pi(&s, &["dns", "zone", "list", "-o", "wide"]).await.out);
    // Wide is the narrow table plus its six columns.
    assert_eq!(
        wide.headers.len() - narrow.headers.len(),
        6,
        "{:?}",
        wide.headers
    );
    // And a row whose trailing columns are empty still keeps its place: the
    // renderer trims the line, so the check is by header index, which is how
    // the table must be read at all.
    assert_eq!(wide.cell("sec", "KIND"), "secondary");
    assert_eq!(wide.cell("sec", "PRIMARY"), "198.51.100.9");
    assert_eq!(wide.cell("sec", "NOTIFY"), "");
    assert_eq!(wide.cell("example", "NOTIFY"), "");
    assert_eq!(wide.cell("example", "FULL-ID"), example);

    // A record set with no TTL, in a zone the listing also carries.
    let (status, v) = s
        .post(
            "/dns/recordsets",
            json!({
                "name": "hole",
                "spec": {"zone": corp, "name": "h", "type": "A", "records": ["10.9.0.8"]},
            }),
        )
        .await;
    assert_eq!(status, StatusCode::CREATED, "{v}");
    let r = pi(&s, &["dns", "record", "list", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let wide = Table::new(&r.out);
    assert_eq!(wide.cell("hole", "TTL"), "3600");
    assert_eq!(wide.cell("hole", "OWNER"), "h.corp.cloud.");
    // A --zone walk still shows only that zone's sets.
    let r = pi(&s, &["dns", "record", "list", "--zone", &example]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(!r.out.contains("hole"), "{}", r.out);
    assert!(r.out.contains("web-a"), "{}", r.out);
}

/// A zone is only useful once it serves. Until the reconciler runs, the table
/// says what the module says: pending, serial zero, nothing listed inside.
#[tokio::test]
async fn a_fresh_zone_is_pending_until_reconciled() {
    let s = serve().await;
    let r = pi(&s, &["dns", "zone", "create", "new.example"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let r = pi(&s, &["dns", "zone", "show", "new.example", "-o", "json"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["status"]["phase"], "pending");
    assert_eq!(v["status"]["serial"], 0);
    let r = pi(&s, &["dns", "zone", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("pending"), "{}", r.out);
    let r = pi(&s, &["dns", "record", "list", "--zone", "new.example"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(!r.out.contains("new.example"), "{}", r.out);
}

/// The CLI's rows are the store's rows: byte for byte where the document is
/// concerned. This is the check that no fixture in this file has quietly
/// become the contract.
#[tokio::test]
async fn the_cli_shows_exactly_what_the_store_holds() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    let stored = s.records();
    let r = pi(&s, &["dns", "record", "list", "-o", "json"]).await;
    let shown: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(shown.len(), stored.len());
    for (name, row) in &stored {
        let it = shown
            .iter()
            .find(|it| it["name"] == name.as_str())
            .unwrap_or_else(|| panic!("{name} not shown"));
        assert_eq!(it, row, "{name} as the store holds it");
    }
    let r = pi(&s, &["dns", "zone", "list", "-o", "json"]).await;
    let shown: Vec<Value> = serde_json::from_str(&r.out).unwrap();
    assert_eq!(shown, s.zone_rows());
    // The record points at the zone the CLI resolved, and nowhere else.
    assert_eq!(s.record("web-a")["spec"]["zone"], example);
}

/// A zone write moves the zone, and the CLI reads the row the module wrote —
/// including the update path, which the CLI has no verb for yet: what it
/// sends on create must survive a round trip through the module's own PUT.
#[tokio::test]
async fn a_zone_body_round_trips_through_the_module_s_update() {
    let s = serve().await;
    let (example, _) = seed_pair(&s).await;
    // The body the CLI builds for `zone create`, sent to PUT as an admin
    // would repair one by hand.
    let (status, v) = s
        .put(
            &format!("/dns/zones/{example}"),
            json!({
                "spec": {
                    "name": "example.cloud.",
                    "kind": {"type": "primary"},
                    "ttl": 900,
                    "email": "ops@example.cloud",
                    "nameservers": ["ns1.example.cloud."],
                    "also_notify": ["203.0.113.77"],
                },
            }),
        )
        .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    assert_eq!(v["spec"]["ttl"], 900);
    let wide = Table::new(&pi(&s, &["dns", "zone", "list", "-o", "wide"]).await.out);
    assert_eq!(wide.cell("example", "TTL"), "900");
    assert_eq!(wide.cell("example", "NOTIFY"), "203.0.113.77");
    assert_eq!(wide.cell("example", "NAMESERVERS"), "ns1.example.cloud.");
}

#[test]
fn the_suite_mounts_the_module_it_targets() {
    // A guard, not a behaviour test: this file must talk to the module whose
    // router the gateway mounts, at the prefix the CLI's paths assume. If
    // either moves, every test above stops proving anything about `pi dns`.
    use pistack_types::Module as _;
    assert_eq!(Beacon::default().info().prefix, "dns");
    assert_eq!(dns::ZONE.path, "/dns/zones");
    assert_eq!(dns::RECORD.path, "/dns/recordsets");
}
