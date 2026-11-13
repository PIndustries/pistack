// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Drive `bench::cli::run` against a tiny in-process axum server that
//! speaks just enough of the PiStack API: `/identity/tokens`, `/compute/
//! machines` and `/identity/tokens/self`.

use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    routing::{get, post},
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const TOKEN: &str = "tok-secret-123";

#[derive(Clone, Default)]
struct Mock {
    machines: Arc<Mutex<BTreeMap<String, Value>>>,
    polls: Arc<Mutex<u32>>,
    pods: Arc<Mutex<BTreeMap<String, Value>>>,
    execs: Arc<Mutex<Vec<Value>>>,
    /// Each server removed, and the `force` it was asked with.
    removals: Arc<Mutex<Vec<(String, bool)>>>,
    quotas: Arc<Mutex<BTreeMap<String, u64>>>,
    networks: Arc<Mutex<BTreeMap<String, Value>>>,
}

fn err(status: StatusCode, code: &str, msg: &str) -> axum::response::Response {
    (
        status,
        Json(json!({"error": {"code": code, "message": msg, "details": {}}})),
    )
        .into_response()
}

fn authed(h: &HeaderMap) -> bool {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == format!("Bearer {TOKEN}"))
}

async fn login(Json(body): Json<Value>) -> axum::response::Response {
    if body["name"] == "admin" && body["password"] == "x" {
        Json(json!({
            "token": TOKEN,
            "token_type": "Bearer",
            "expires_at": "2030-01-01T00:00:00Z",
            "caller": {
                "principal": "prin-0199a5e0-1111-7000-8000-000000000001",
                "principal_name": "admin",
                "project": "proj-0199a5e0-2222-7000-8000-000000000002",
                "roles": ["admin"],
                "system": true
            }
        }))
        .into_response()
    } else {
        err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "invalid credentials",
        )
    }
}

async fn whoami(h: HeaderMap) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    Json(json!({
        "principal": "prin-0199a5e0-1111-7000-8000-000000000001",
        "principal_name": "admin",
        "project": null,
        "roles": ["admin"],
        "system": true
    }))
    .into_response()
}

async fn list(State(m): State<Mock>, h: HeaderMap) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    let items: Vec<Value> = m.machines.lock().unwrap().values().cloned().collect();
    let total = items.len();
    Json(json!({"items": items, "total": total})).into_response()
}

async fn create(
    State(m): State<Mock>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    let name = body["name"].as_str().unwrap_or("").to_owned();
    if body["spec"]["size"].as_str().unwrap_or("").is_empty() {
        return err(
            StatusCode::BAD_REQUEST,
            "bad_request",
            "spec.size is required",
        );
    }
    if body["spec"]["size"] == "huge" {
        return err(
            StatusCode::UNPROCESSABLE_ENTITY,
            "unprocessable",
            "size 'huge' not found",
        );
    }
    let mut machines = m.machines.lock().unwrap();
    if machines.contains_key(&name) {
        return err(StatusCode::CONFLICT, "conflict", "machine already exists");
    }
    let now = chrono::Utc::now().to_rfc3339();
    let v = json!({
        "id": format!("mach-0199a5e0-aaaa-7000-8000-{:012}", machines.len() + 1),
        "name": name,
        "project": "proj-0199a5e0-2222-7000-8000-000000000002",
        "created_at": now, "updated_at": now, "revision": 1,
        "spec": body["spec"],
        "status": {"phase": "pending", "power": "unknown", "addresses": []}
    });
    machines.insert(name, v.clone());
    (StatusCode::CREATED, Json(v)).into_response()
}

fn find(machines: &BTreeMap<String, Value>, id: &str) -> Option<Value> {
    machines
        .get(id)
        .cloned()
        .or_else(|| machines.values().find(|v| v["id"] == id).cloned())
}

/// Each GET moves the machine one phase along: pending -> creating -> ready.
async fn show(
    State(m): State<Mock>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    let mut machines = m.machines.lock().unwrap();
    let Some(mut v) = find(&machines, &id) else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("mach '{id}' not found"),
        );
    };
    *m.polls.lock().unwrap() += 1;
    let name = v["name"].as_str().unwrap_or("").to_owned();
    let next = match v["status"]["phase"].as_str() {
        Some("pending") => "creating".to_owned(),
        Some("creating") => "ready".to_owned(),
        other => other.unwrap_or("ready").to_owned(),
    };
    v["status"]["phase"] = json!(next);
    if next == "ready" {
        v["status"]["power"] = json!("on");
        v["status"]["addresses"] = json!(["10.1.0.7"]);
    }
    machines.insert(name, v.clone());
    Json(v).into_response()
}

async fn delete(
    State(m): State<Mock>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    let mut machines = m.machines.lock().unwrap();
    let Some(v) = find(&machines, &id) else {
        return err(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("mach '{id}' not found"),
        );
    };
    machines.remove(v["name"].as_str().unwrap_or(""));
    (StatusCode::ACCEPTED, Json(v)).into_response()
}

async fn start(State(m): State<Mock>, Path(id): Path<String>) -> axum::response::Response {
    let machines = m.machines.lock().unwrap();
    match find(&machines, &id) {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::NOT_FOUND, "not_found", "no such machine"),
    }
}

fn find_pod(pods: &BTreeMap<String, Value>, id: &str) -> Option<Value> {
    pods.get(id).cloned().or_else(|| {
        pods.values()
            .find(|v| v["id"] == id || v["name"] == id)
            .cloned()
    })
}

async fn show_pod(
    State(m): State<Mock>,
    h: HeaderMap,
    Path(id): Path<String>,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    match find_pod(&m.pods.lock().unwrap(), &id) {
        Some(v) => Json(v).into_response(),
        None => err(
            StatusCode::NOT_FOUND,
            "not_found",
            &format!("pod '{id}' not found"),
        ),
    }
}

async fn exec_pod(
    State(m): State<Mock>,
    h: HeaderMap,
    Path(id): Path<String>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    m.execs
        .lock()
        .unwrap()
        .push(json!({"pod": id, "body": body}));
    let failed = body["command"][0] == "false";
    Json(json!({
        "exit_code": if failed { 3 } else { 0 },
        "stdout": if failed { "nope\n" } else { "hi\n" },
        "stderr": if failed { "bad\n" } else { "" }
    }))
    .into_response()
}

async fn remove_server(
    State(m): State<Mock>,
    h: HeaderMap,
    Path(id): Path<String>,
    axum::extract::Query(q): axum::extract::Query<BTreeMap<String, String>>,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    let force = q.get("force").is_some_and(|v| v == "true");
    if id == "srv-c" {
        return err(
            StatusCode::CONFLICT,
            "conflict",
            "this server is the leader",
        );
    }
    if id == "srv-b" && !force {
        return err(StatusCode::CONFLICT, "conflict", "not a majority");
    }
    m.removals.lock().unwrap().push((id, force));
    Json(json!({"members": []})).into_response()
}

async fn discovery() -> Json<Value> {
    Json(json!({
        "name": "pistack", "version": "26.11.12.1", "public_url": "http://mock", "dev_mode": false,
        "modules": [
            {"name": "gate", "prefix": "identity", "summary": "Identity", "version": "26.11.12.1", "url": "http://mock/identity"},
            {"name": "forge", "prefix": "compute", "summary": "Machines", "version": "26.11.12.1", "url": "http://mock/compute"}
        ]
    }))
}

async fn cluster_status(h: HeaderMap) -> axum::response::Response {
    if !authed(&h) {
        return err(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "cluster routes need a member or a join token",
        );
    }
    Json(json!({
        "cluster_id": "c1",
        "node_id": "srv-b",
        "api": "http://10.0.0.12:8443",
        "leader_api": "http://10.0.0.13:8443",
        "leader_id": "srv-c",
        "role": "Voter",
        "leader": false,
        "members": [
            {"id": "srv-a", "api": "http://10.0.0.11:8443", "role": "Voter",
             "agent_api": "https://10.0.0.11:8444"},
            {"id": "srv-b", "api": "http://10.0.0.12:8443", "role": "Voter",
             "agent_api": "https://10.0.0.12:8444"},
            {"id": "srv-c", "api": "http://10.0.0.13:8443", "role": "Voter",
             "agent_api": "https://10.0.0.13:8444"},
            {"id": "srv-d", "api": "http://10.0.0.14:8443", "role": "Learner"}
        ],
        "term": 7,
        "commit": 74
    }))
    .into_response()
}

/// The report gate answers with: every limit set, plus `vcpus` usage.
fn quota_report(m: &Mock) -> Value {
    let mut lines = serde_json::Map::new();
    lines.insert("vcpus".into(), json!({"limit": null, "used": 6}));
    lines.insert("volumes_gib".into(), json!({"limit": null, "used": 0}));
    for (resource, limit) in m.quotas.lock().unwrap().iter() {
        let used = lines[resource]["used"].clone();
        lines.insert(resource.clone(), json!({"limit": limit, "used": used}));
    }
    json!({
        "project": "proj-0199a5e0-2222-7000-8000-000000000002",
        "name": "web",
        "quotas": lines,
    })
}

async fn show_quotas(
    State(m): State<Mock>,
    Path(id): Path<String>,
    h: HeaderMap,
) -> axum::response::Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "unauthorized", "no credentials");
    }
    if id != "web" {
        return err(StatusCode::NOT_FOUND, "not_found", "project not found");
    }
    Json(quota_report(&m)).into_response()
}

async fn set_quotas(
    State(m): State<Mock>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    if !authed(&h) || id != "web" {
        return err(StatusCode::NOT_FOUND, "not_found", "project not found");
    }
    for (resource, limit) in body["quotas"].as_object().unwrap() {
        if resource != "vcpus" && resource != "volumes_gib" {
            return err(
                StatusCode::BAD_REQUEST,
                "bad_request",
                &format!("unknown quota resource '{resource}'"),
            );
        }
        let mut quotas = m.quotas.lock().unwrap();
        match limit.as_u64() {
            Some(n) => quotas.insert(resource.clone(), n),
            None => quotas.remove(resource),
        };
    }
    Json(quota_report(&m)).into_response()
}

async fn create_network(
    State(m): State<Mock>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let name = body["name"].as_str().unwrap().to_owned();
    let row = json!({
        "id": "net-00000001",
        "name": name,
        "revision": 1,
        "spec": body["spec"],
        "status": {"phase": "ready"},
    });
    m.networks.lock().unwrap().insert(name, row.clone());
    (StatusCode::CREATED, Json(row)).into_response()
}

async fn show_network(State(m): State<Mock>, Path(id): Path<String>) -> axum::response::Response {
    match m.networks.lock().unwrap().get(&id) {
        Some(row) => Json(row.clone()).into_response(),
        None => err(StatusCode::NOT_FOUND, "not_found", "network not found"),
    }
}

/// Replaces the spec, as the server does, and insists on the revision.
async fn update_network(
    State(m): State<Mock>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let mut networks = m.networks.lock().unwrap();
    let Some(row) = networks.get_mut(&id) else {
        return err(StatusCode::NOT_FOUND, "not_found", "network not found");
    };
    let sent = h.get("if-match").and_then(|v| v.to_str().ok());
    if sent.map(|v| v.trim_matches('"')) != Some(row["revision"].to_string().as_str()) {
        return err(StatusCode::CONFLICT, "conflict", "revision mismatch");
    }
    row["spec"] = body["spec"].clone();
    row["revision"] = json!(row["revision"].as_u64().unwrap() + 1);
    Json(row.clone()).into_response()
}

#[tokio::test]
async fn automatic_floating_ips_are_set_on_create_and_changed_later() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let r = pi(
        &cfg,
        "x\n",
        &["login", &url, "--name", "admin", "--password-stdin"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");

    // The flag makes no sense on a network that is not external.
    let r = pi(&cfg, "", &["network", "create", "in", "--auto-floating-ip"]).await;
    assert_eq!(r.code, 2, "{r:?}");

    let r = pi(
        &cfg,
        "",
        &["network", "create", "public", "--external", "--mtu", "1400"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let spec = |m: &Mock| m.networks.lock().unwrap()["public"]["spec"].clone();
    assert_eq!(spec(&mock)["auto_floating_ip"], false);

    let r = pi(
        &cfg,
        "",
        &["network", "set", "public", "--auto-floating-ip", "true"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(spec(&mock)["auto_floating_ip"], true);
    // The rest of the spec went back as it was.
    assert_eq!(spec(&mock)["external"], true);
    assert_eq!(spec(&mock)["mtu"], 1400);

    let r = pi(
        &cfg,
        "",
        &["network", "set", "public", "--auto-floating-ip", "false"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(spec(&mock)["auto_floating_ip"], false);

    // Saying nothing is a usage error, not a silent no-op.
    let r = pi(&cfg, "", &["network", "set", "public"]).await;
    assert_eq!(r.code, 2, "{r:?}");

    let r = pi(
        &cfg,
        "",
        &[
            "network",
            "create",
            "edge",
            "--external",
            "--auto-floating-ip",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        mock.networks.lock().unwrap()["edge"]["spec"]["auto_floating_ip"],
        true
    );
}

async fn serve() -> (String, Mock) {
    let mock = Mock::default();
    let app = Router::new()
        .route("/", get(discovery))
        .route("/identity/tokens", post(login))
        .route("/identity/tokens/self", get(whoami))
        .route("/cluster", get(cluster_status))
        .route(
            "/identity/projects/{id}/quotas",
            get(show_quotas).put(set_quotas),
        )
        .route("/network/networks", post(create_network))
        .route(
            "/network/networks/{id}",
            get(show_network).put(update_network),
        )
        .route("/compute/machines", get(list).post(create))
        .route("/compute/machines/{id}", get(show).delete(delete))
        .route("/compute/machines/{id}/start", post(start))
        .route("/loom/pods/{id}", get(show_pod))
        .route("/loom/pods/{id}/exec", post(exec_pod))
        .route(
            "/cluster/members/{id}",
            axum::routing::delete(remove_server),
        )
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), mock)
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(config: &std::path::Path, stdin: &str, args: &[&str]) -> Run {
    let mut argv = vec![
        "pi".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
    ];
    argv.extend(args.iter().map(|s| s.to_string()));
    let mut input = Cursor::new(stdin.as_bytes().to_vec());
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

#[tokio::test]
async fn login_then_machines_end_to_end() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");

    // Unauthenticated use is a usage error with a login hint.
    let r = pi(&cfg, "", &["machine", "list"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.err.starts_with("error: no server configured"),
        "{}",
        r.err
    );

    // Wrong password: the error envelope is parsed and printed.
    let r = pi(
        &cfg,
        "wrong\n",
        &["login", &url, "--name", "admin", "--password-stdin"],
    )
    .await;
    assert_eq!(r.code, 1);
    assert_eq!(r.err.lines().next().unwrap(), "error: invalid credentials");
    assert!(
        r.err.contains("hint: your token is missing or expired"),
        "{}",
        r.err
    );

    // Login stores the token under the default profile.
    let r = pi(
        &cfg,
        "x\n",
        &["login", &url, "--name", "admin", "--password-stdin"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.out.contains("logged in to") && r.out.contains("as admin"),
        "{}",
        r.out
    );
    let saved = cli::Config::load(&cfg).unwrap();
    assert_eq!(saved.current().unwrap().token.as_deref(), Some(TOKEN));
    assert_eq!(saved.current().unwrap().url, url);

    let r = pi(&cfg, "", &["profile", "list"]).await;
    assert_eq!(r.code, 0);
    assert!(
        r.out.contains("default") && r.out.contains(&url),
        "{}",
        r.out
    );

    let r = pi(&cfg, "", &["whoami"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("principal  admin"), "{}", r.out);

    let r = pi(&cfg, "", &["modules"]).await;
    assert_eq!(r.code, 0);
    assert!(
        r.out.contains("forge") && r.out.contains("/compute"),
        "{}",
        r.out
    );

    // Empty list: only the header.
    let r = pi(&cfg, "", &["machine", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.lines().count(), 1);
    assert!(r.out.starts_with("NAME  ID  PHASE"), "{}", r.out);

    // Create with --wait prints one line per phase change.
    let r = pi(
        &cfg,
        "",
        &[
            "machine",
            "create",
            "web",
            "--size",
            "m.2x4",
            "--image",
            "ubuntu",
            "--network",
            "lan",
            "--kind",
            "container",
            "--label",
            "tier=web",
            "--wait",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<&str> = r.out.lines().collect();
    assert!(
        lines[0].starts_with("machine \"web\" created (mach-00000001)"),
        "{}",
        r.out
    );
    assert_eq!(lines[1], "machine \"web\": creating");
    assert_eq!(lines[2], "machine \"web\": ready");
    assert_eq!(lines.len(), 3);
    let stored = mock.machines.lock().unwrap()["web"].clone();
    assert_eq!(stored["spec"]["kind"], "container");
    assert_eq!(stored["spec"]["networks"], json!(["lan"]));
    assert!(
        stored["spec"].get("user_data").is_none(),
        "nulls must be dropped"
    );

    // Table and name output.
    let r = pi(&cfg, "", &["machine", "list"]).await;
    assert_eq!(r.code, 0);
    assert!(
        r.out
            .lines()
            .nth(1)
            .unwrap()
            .starts_with("web   mach-00000001  ready"),
        "{}",
        r.out
    );
    assert!(r.out.contains("10.1.0.7"));
    let r = pi(&cfg, "", &["machine", "list", "-o", "name"]).await;
    assert_eq!(r.out, "web\n");
    let r = pi(&cfg, "", &["-o", "json", "machine", "show", "web"]).await;
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["name"], "web");

    // Server-side validation error -> exit 1, message on stderr, nothing on stdout.
    let r = pi(
        &cfg,
        "",
        &["machine", "create", "big", "--size", "huge", "--image", "u"],
    )
    .await;
    assert_eq!(r.code, 1);
    assert_eq!(r.err.trim(), "error: size 'huge' not found");
    assert!(r.out.is_empty());

    // Action endpoint.
    let r = pi(&cfg, "", &["machine", "start", "web"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.out.starts_with("machine \"web\" start requested"),
        "{}",
        r.out
    );

    // Delete without --yes and no tty refuses; with --yes it goes.
    let r = pi(&cfg, "", &["machine", "delete", "web"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("--yes"), "{}", r.err);
    let r = pi(&cfg, "", &["machine", "delete", "web", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "machine \"web\" deletion requested");
    let r = pi(&cfg, "", &["machine", "show", "web"]).await;
    assert_eq!(r.code, 1);
    assert_eq!(r.err.trim(), "error: mach 'web' not found");

    // A bad token is a 401 with the login hint.
    let r = pi(&cfg, "", &["--token", "nope", "machine", "list"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("error: no credentials"), "{}", r.err);
    assert!(r.err.contains(&format!("pi login {url}")), "{}", r.err);

    // Raw API access.
    let r = pi(&cfg, "", &["api", "GET", "/"]).await;
    assert_eq!(r.code, 0);
    assert!(r.out.contains("\"name\": \"pistack\""));
    let r = pi(&cfg, "", &["version"]).await;
    assert_eq!(r.code, 0);
    assert!(
        r.out.contains(&format!("pi {}", pistack_types::BUILD)),
        "{}",
        r.out
    );
    assert!(r.out.contains("pistack 26.11.12.1"), "{}", r.out);
}

#[tokio::test]
async fn apply_creates_then_updates() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let manifest = dir.path().join("m.yaml");
    std::fs::write(
        &manifest,
        "kind: Machine\nname: app\nlabels: {a: b}\nspec:\n  size: s.1x1\n  image: alpine\n",
    )
    .unwrap();
    let r = pi(
        &cfg,
        "x\n",
        &["login", &url, "--name", "admin", "--password-stdin"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&cfg, "", &["apply", "-f", manifest.to_str().unwrap()]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.starts_with("machine \"app\" created"), "{}", r.out);
    assert_eq!(
        mock.machines.lock().unwrap()["app"]["spec"]["image"],
        "alpine"
    );
    // Manifest from stdin, unknown kind is rejected cleanly.
    let r = pi(&cfg, "kind: widget\nname: w\n", &["apply", "-f", "-"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("unknown kind 'widget'"), "{}", r.err);
}

#[tokio::test]
async fn env_and_flags_override_profile() {
    let (url, _mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    // No profile at all: --url and --token are enough.
    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "machine",
            "list",
            "--no-headers",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.is_empty(), "{}", r.out);
}

#[tokio::test]
async fn cluster_status_lists_the_servers_and_marks_the_leader() {
    let (url, _mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let r = pi(&cfg, "", &["--url", &url, "cluster", "status"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("pi login"), "{}", r.err);

    let admin = ["--url", url.as_str(), "--token", TOKEN];
    let r = pi(&cfg, "", &[&admin[..], &["cluster", "status"]].concat()).await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<Vec<&str>> = r
        .out
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(
        lines[0],
        ["SERVER", "ROLE", "LEADER", "API", "AGENT", "API"]
    );
    assert_eq!(
        lines[1],
        [
            "srv-a",
            "voter",
            "http://10.0.0.11:8443",
            "https://10.0.0.11:8444"
        ]
    );
    assert_eq!(
        lines[3],
        [
            "srv-c",
            "voter",
            "yes",
            "http://10.0.0.13:8443",
            "https://10.0.0.13:8444"
        ]
    );
    assert_eq!(lines[4], ["srv-d", "learner", "http://10.0.0.14:8443"]);
    assert_eq!(lines.len(), 5);
    assert_eq!(r.err, "");

    let r = pi(
        &cfg,
        "",
        &[&admin[..], &["cluster", "status", "-o", "json"]].concat(),
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["leader_id"], "srv-c");
    assert_eq!(v["members"].as_array().unwrap().len(), 4);
}

/// A server that refuses every write the way a follower does, naming
/// `leader`, and counts what it was sent.
async fn follower(leader: String) -> (String, Arc<Mutex<u32>>) {
    let refused = Arc::new(Mutex::new(0));
    let count = refused.clone();
    let app = Router::new().fallback(move || {
        let leader = leader.clone();
        let count = count.clone();
        async move {
            *count.lock().unwrap() += 1;
            (
                StatusCode::CONFLICT,
                Json(
                    json!({"error": {"code": "conflict", "message": "not the leader",
                    "details": {"leader": leader}}}),
                ),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), refused)
}

#[tokio::test]
async fn a_write_refused_by_a_follower_is_sent_to_the_leader_once() {
    let (leader, mock) = serve().await;
    let (url, refused) = follower(leader.clone()).await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let create = [
        "machine", "create", "web", "--size", "s.1x1", "--image", "a",
    ];

    // The leader checks the token, so the retry carried it.
    let on = ["--url", url.as_str(), "--token", TOKEN];
    let r = pi(&cfg, "", &[&on[..], &create[..]].concat()).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(mock.machines.lock().unwrap().contains_key("web"));
    assert_eq!(*refused.lock().unwrap(), 1);
    assert_eq!(
        r.err,
        format!("{url} is not the leader; sending the request to {leader}\n")
    );
    assert!(r.out.contains("web"), "{}", r.out);

    // The leader's own answer comes back as it is, with the notice.
    let r = pi(&cfg, "", &[&on[..], &create[..]].concat()).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("is not the leader; sending"), "{}", r.err);
    assert!(r.err.contains("error: machine already exists"), "{}", r.err);

    // Two followers that name each other: one retry, then the refusal.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let other = format!("http://{}", listener.local_addr().unwrap());
    let (lost, lost_refused) = follower(other.clone()).await;
    let back = lost.clone();
    let app = Router::new().fallback(move || {
        let back = back.clone();
        async move {
            (
                StatusCode::CONFLICT,
                Json(
                    json!({"error": {"code": "conflict", "message": "not the leader",
                    "details": {"leader": back}}}),
                ),
            )
        }
    });
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let on = ["--url", lost.as_str(), "--token", TOKEN];
    let r = pi(&cfg, "", &[&on[..], &create[..]].concat()).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert_eq!(*lost_refused.lock().unwrap(), 1);
    assert!(r.err.contains("error: not the leader"), "{}", r.err);
    assert!(
        r.err.contains(&format!("\"leader\":\"{lost}\"")),
        "{}",
        r.err
    );

    // A leader on another scheme is not followed: the token stays put.
    let (url, refused) = follower("https://127.0.0.1:1".into()).await;
    let on = ["--url", url.as_str(), "--token", TOKEN];
    let r = pi(&cfg, "", &[&on[..], &create[..]].concat()).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert_eq!(*refused.lock().unwrap(), 1);
    assert!(r.err.starts_with("error: not the leader"), "{}", r.err);
    assert!(r.err.contains("https://127.0.0.1:1"), "{}", r.err);
}

#[tokio::test]
async fn usage_errors_and_completion() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let r = pi(&cfg, "", &["machine", "create"]).await;
    assert_eq!(r.code, 2);
    assert!(r.err.contains("--size"), "{}", r.err);
    let r = pi(&cfg, "", &["--help"]).await;
    assert_eq!(r.code, 0);
    assert!(r.out.contains("pi login"), "{}", r.out);
    let r = pi(&cfg, "", &["completion", "bash"]).await;
    assert_eq!(r.code, 0);
    assert!(r.out.contains("_pi"), "{}", r.out);
    let r = pi(&cfg, "", &["pod", "exec", "--help"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("-c"), "{}", r.out);
    assert!(r.out.contains("--timeout"), "{}", r.out);
}

#[tokio::test]
async fn pod_exec_prints_output_and_returns_the_exit_code() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    mock.pods.lock().unwrap().insert(
        "web".into(),
        json!({
            "id": "pod-web",
            "name": "web",
            "spec": {"containers": [{"name": "app", "image": "img"}]}
        }),
    );
    mock.pods.lock().unwrap().insert(
        "multi".into(),
        json!({
            "id": "pod-multi",
            "name": "multi",
            "spec": {"containers": [
                {"name": "app", "image": "img"},
                {"name": "side", "image": "img"}
            ]}
        }),
    );
    mock.pods.lock().unwrap().insert(
        "bare".into(),
        json!({"id": "pod-bare", "name": "bare", "spec": {"containers": []}}),
    );

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "pod",
            "exec",
            "multi",
            "--",
            "/bin/echo",
            "hi",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("app"), "{}", r.err);
    assert!(r.err.contains("side"), "{}", r.err);
    assert!(mock.execs.lock().unwrap().is_empty(), "usage must not exec");

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "pod",
            "exec",
            "multi",
            "-c",
            "missing",
            "--",
            "/bin/true",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("missing"), "{}", r.err);
    assert!(mock.execs.lock().unwrap().is_empty());

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "pod",
            "exec",
            "bare",
            "--",
            "/bin/true",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("pass -c"), "{}", r.err);
    assert!(mock.execs.lock().unwrap().is_empty());

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "pod",
            "exec",
            "web",
            "--timeout",
            "12",
            "--",
            "/bin/echo",
            "hi",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "hi\n");
    assert!(r.err.is_empty(), "{}", r.err);
    let execs = mock.execs.lock().unwrap().clone();
    assert_eq!(execs.len(), 1);
    assert_eq!(execs[0]["pod"], "pod-web");
    assert_eq!(execs[0]["body"]["container"], "app");
    assert_eq!(execs[0]["body"]["command"], json!(["/bin/echo", "hi"]));
    assert_eq!(execs[0]["body"]["timeout_secs"], 12);

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "pod",
            "exec",
            "web",
            "--",
            "/bin/echo",
            "--hello",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        mock.execs.lock().unwrap().last().unwrap()["body"]["command"],
        json!(["/bin/echo", "--hello"])
    );

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "-o",
            "json",
            "pod",
            "exec",
            "multi",
            "-c",
            "side",
            "--",
            "/bin/echo",
            "hi",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("\"exit_code\""), "{}", r.out);
    assert!(r.out.contains("hi"), "{}", r.out);
    assert_eq!(
        mock.execs.lock().unwrap().last().unwrap()["body"]["container"],
        "side"
    );

    let r = pi(
        &cfg,
        "",
        &[
            "--url", &url, "--token", TOKEN, "pod", "exec", "web", "--", "false",
        ],
    )
    .await;
    assert_eq!(r.code, 3, "{r:?}");
    assert_eq!(r.out, "nope\n");
    assert_eq!(r.err, "bad\n");
    assert!(!r.err.contains("error:"), "{}", r.err);

    let r = pi(
        &cfg,
        "",
        &[
            "--url",
            &url,
            "--token",
            TOKEN,
            "pod",
            "exec",
            "bare",
            "-c",
            "app",
            "--",
            "/bin/true",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        mock.execs.lock().unwrap().last().unwrap()["body"]["container"],
        "app"
    );
}

#[tokio::test]
async fn a_server_is_removed_by_id_prefix_or_address() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let admin = ["--url", url.as_str(), "--token", TOKEN, "server"];
    let run = |rest: &[&'static str]| {
        let args = [&admin[..], rest].concat();
        let cfg = cfg.clone();
        async move { pi(&cfg, "", &args).await }
    };

    // No terminal to confirm on, and nothing is sent.
    let r = run(&["remove", "http://10.0.0.14:8443"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("--yes"), "{}", r.err);
    // A name that is no server, and one that is four.
    let r = run(&["remove", "nope", "-y"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("no server 'nope'"), "{}", r.err);
    let r = run(&["remove", "srv-", "-y"]).await;
    assert!(r.err.contains("more than one"), "{}", r.err);
    assert!(mock.removals.lock().unwrap().is_empty());

    // By address, then what the server refuses, then forced by id.
    let r = run(&["remove", "http://10.0.0.14:8443", "-y"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "server srv-d (http://10.0.0.14:8443) removed\n");
    let r = run(&["rm", "srv-b", "--yes"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not a majority"), "{}", r.err);
    let r = run(&["rm", "srv-c", "--yes", "--force"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("is the leader"), "{}", r.err);
    let r = run(&["rm", "srv-b", "--yes", "--force"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        *mock.removals.lock().unwrap(),
        [("srv-d".to_owned(), false), ("srv-b".to_owned(), true)]
    );
}

#[tokio::test]
async fn quota_set_then_show() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config.toml");
    let r = pi(
        &cfg,
        "x\n",
        &["login", &url, "--name", "admin", "--password-stdin"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");

    // Set two limits; the answer is the project's report.
    let r = pi(
        &cfg,
        "",
        &["quota", "set", "web", "vcpus=16", "volumes_gib=500"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(mock.quotas.lock().unwrap()["vcpus"], 16);
    let lines: Vec<Vec<&str>> = r
        .out
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(lines[0], ["RESOURCE", "LIMIT", "USED"]);
    assert_eq!(lines[1], ["vcpus", "16", "6"]);
    assert_eq!(lines[2], ["volumes_gib", "500", "0"]);

    // `unlimited` removes one and leaves the other.
    let r = pi(&cfg, "", &["quota", "set", "web", "volumes_gib=unlimited"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(!mock.quotas.lock().unwrap().contains_key("volumes_gib"));
    assert_eq!(mock.quotas.lock().unwrap()["vcpus"], 16);

    let r = pi(&cfg, "", &["quota", "show", "web"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<Vec<&str>> = r
        .out
        .lines()
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(lines[1], ["vcpus", "16", "6"]);
    assert_eq!(lines[2], ["volumes_gib", "-", "0"]);

    let r = pi(&cfg, "", &["quota", "show", "web", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["quotas"]["vcpus"], json!({"limit": 16, "used": 6}));

    // The global project is the default.
    let r = pi(&cfg, "", &["--project", "web", "quota", "show"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("vcpus"), "{}", r.out);

    // The server names a resource it does not know.
    let r = pi(&cfg, "", &["quota", "set", "web", "vcpu=4"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("unknown quota resource 'vcpu'"), "{}", r.err);

    // Not a number, and nothing to set.
    let r = pi(&cfg, "", &["quota", "set", "web", "vcpus=lots"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not a number"), "{}", r.err);
    let r = pi(&cfg, "", &["quota", "set", "web"]).await;
    assert_eq!(r.code, 2, "{r:?}");
}
