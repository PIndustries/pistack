// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi share` against a small in-process server that keeps shares the way
//! shelf does: one collection, `POST` to create, `PUT` with `If-Match` to
//! replace the spec, `DELETE` to drop it.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const ID: &str = "share-0199a5e0-1111-7000-8000-00000000000a";

/// One `PUT` as it reached the server: the share, the `If-Match`, the body.
type Put = (String, Option<String>, Value);

#[derive(Clone, Default)]
struct Shelf {
    shares: Arc<Mutex<BTreeMap<String, Value>>>,
    /// Every `PUT` the CLI made, with the `If-Match` it carried.
    puts: Arc<Mutex<Vec<Put>>>,
    deletes: Arc<Mutex<Vec<String>>>,
    queries: Arc<Mutex<Vec<String>>>,
    /// `Some` once per run: the next read of a share is answered with what
    /// this holds, while the collection is left as it was. The command that
    /// asked therefore writes against a revision that is already stale.
    stale_read: Arc<Mutex<Option<Value>>>,
}

fn err(status: StatusCode, msg: &str) -> Response {
    (
        status,
        Json(json!({"error": {"code": "error", "message": msg, "details": {}}})),
    )
        .into_response()
}

fn authed(h: &HeaderMap) -> bool {
    let ok = h
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| v == "Bearer t");
    ok
}

/// What shelf hands back for a share: the meta fields flattened beside the
/// spec and status.
fn stored(name: &str, spec: Value, revision: u64) -> Value {
    json!({
        "id": ID,
        "name": name,
        "project": "proj-0199a5e0-2222-7000-8000-000000000002",
        "created_at": "2026-11-14T08:00:00Z",
        "updated_at": "2026-11-14T08:00:00Z",
        "revision": revision,
        "spec": spec,
        "status": {"phase": "pending", "message": "this build has no real NFS/Ceph daemon"}
    })
}

async fn list(
    State(s): State<Shelf>,
    h: HeaderMap,
    Query(q): Query<BTreeMap<String, String>>,
) -> Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "no credentials");
    }
    s.queries.lock().unwrap().push(
        q.iter()
            .map(|(k, v)| format!("{k}={v}"))
            .collect::<Vec<_>>()
            .join("&"),
    );
    let items: Vec<Value> = s.shares.lock().unwrap().values().cloned().collect();
    let total = items.len();
    Json(json!({"items": items, "total": total})).into_response()
}

async fn create(State(s): State<Shelf>, h: HeaderMap, Json(body): Json<Value>) -> Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "no credentials");
    }
    let name = body["name"].as_str().unwrap_or("").to_owned();
    if name.is_empty() {
        return err(StatusCode::BAD_REQUEST, "name is required");
    }
    let mut spec = body["spec"].clone();
    spec["protocol"] = json!(spec["protocol"].as_str().unwrap_or("").to_ascii_lowercase());
    // The server's own rules, so a bad create is refused the way shelf refuses it.
    if !matches!(spec["protocol"].as_str(), Some("nfs") | Some("cephfs")) {
        return err(
            StatusCode::BAD_REQUEST,
            "unknown protocol; expected \"nfs\" or \"cephfs\"",
        );
    }
    if spec["size_gib"].as_u64().unwrap_or(0) == 0 {
        return err(StatusCode::BAD_REQUEST, "size_gib must be greater than 0");
    }
    for (i, rule) in spec["access"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .enumerate()
    {
        let mode = rule["mode"].as_str().unwrap_or("");
        if !matches!(mode, "read" | "write") {
            return err(
                StatusCode::BAD_REQUEST,
                &format!("access[{i}] mode must be read or write"),
            );
        }
        let cidr = rule["cidr"].as_str().unwrap_or("");
        let Some((addr, bits)) = cidr.split_once('/') else {
            return err(
                StatusCode::BAD_REQUEST,
                &format!("access[{i}] cidr '{cidr}' is not a CIDR"),
            );
        };
        if addr.parse::<std::net::IpAddr>().is_err() || bits.parse::<u8>().unwrap_or(255) > 128 {
            return err(
                StatusCode::BAD_REQUEST,
                &format!("access[{i}] cidr '{cidr}' is not a CIDR"),
            );
        }
    }
    let mut share = stored(&name, spec, 1);
    share["description"] = json!(body["description"].as_str().unwrap_or(""));
    share["labels"] = body["labels"].clone();
    let mut shares = s.shares.lock().unwrap();
    if shares.contains_key(&name) {
        return err(StatusCode::CONFLICT, "share already exists");
    }
    shares.insert(name, share.clone());
    (StatusCode::CREATED, Json(share)).into_response()
}

async fn show(State(s): State<Shelf>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "no credentials");
    }
    if let Some(stale) = s.stale_read.lock().unwrap().take() {
        return Json(stale).into_response();
    }
    match find(&s.shares.lock().unwrap(), &id) {
        Some(v) => Json(v).into_response(),
        None => err(StatusCode::NOT_FOUND, &format!("share {id} not found")),
    }
}

async fn update(
    State(s): State<Shelf>,
    h: HeaderMap,
    Path(id): Path<String>,
    headers: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "no credentials");
    }
    let if_match = headers
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_matches('"').to_owned());
    s.puts
        .lock()
        .unwrap()
        .push((id.clone(), if_match.clone(), body.clone()));
    let mut shares = s.shares.lock().unwrap();
    let Some(mut cur) = find(&shares, &id) else {
        return err(StatusCode::NOT_FOUND, &format!("share {id} not found"));
    };
    let rev = cur["revision"].as_u64().unwrap_or(1);
    if let Some(want) = &if_match {
        if want != &rev.to_string() {
            return err(
                StatusCode::CONFLICT,
                &format!("revision mismatch: expected {want}, found {rev}"),
            );
        }
    }
    let spec = body["spec"].clone();
    if spec["size_gib"].as_u64().unwrap_or(0) == 0 {
        return err(StatusCode::BAD_REQUEST, "size_gib must be greater than 0");
    }
    // shelf's update body carries the spec only; the name rides inside it.
    // As shelf does: the spec carries the name, and the record follows it.
    let name = body["spec"]["name"]
        .as_str()
        .filter(|n| !n.is_empty())
        .map(str::to_owned)
        .unwrap_or_else(|| cur["name"].as_str().unwrap_or("").to_owned());
    let mut spec = spec;
    spec["name"] = json!(name);
    cur = stored(&name, spec, rev + 1);
    shares.insert(name, cur.clone());
    Json(cur).into_response()
}

async fn delete(State(s): State<Shelf>, h: HeaderMap, Path(id): Path<String>) -> Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "no credentials");
    }
    let mut shares = s.shares.lock().unwrap();
    let Some(v) = find(&shares, &id) else {
        return err(StatusCode::NOT_FOUND, &format!("share {id} not found"));
    };
    s.deletes.lock().unwrap().push(id);
    shares.remove(v["name"].as_str().unwrap_or(""));
    StatusCode::NO_CONTENT.into_response()
}

/// A share resized by someone else, at a revision ahead of the one in the
/// store. The CLI reads this, then its own write is refused.
fn resized() -> Value {
    let mut moved = stored(
        "home",
        json!({"name": "home", "protocol": "nfs", "size_gib": 20}),
        4,
    );
    moved["spec"] = json!({"name": "home", "protocol": "nfs", "size_gib": 40});
    moved["revision"] = json!(9);
    moved
}

fn find(shares: &BTreeMap<String, Value>, id: &str) -> Option<Value> {
    shares
        .get(id)
        .cloned()
        .or_else(|| shares.values().find(|v| v["id"] == id).cloned())
}

/// With `stale_read`, the first read answers with another writer's version
/// of the share while the store keeps the original.
async fn serve_in(stale_read: bool) -> (String, Shelf) {
    let s = Shelf {
        stale_read: Arc::new(Mutex::new(stale_read.then(resized))),
        ..Shelf::default()
    };
    let app = Router::new()
        .route("/shelf/shares", get(list).post(create))
        .route("/shelf/shares/{id}", get(show).put(update).delete(delete))
        .with_state(s.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), s)
}

async fn serve() -> (String, Shelf) {
    serve_in(false).await
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(url: &str, stdin: &str, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let mut argv: Vec<String> = vec![
        "pi".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
        "--url".to_owned(),
        url.to_owned(),
        "--token".to_owned(),
        "t".to_owned(),
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

/// A share already on the server, with two clients and a principal rule.
fn seed(s: &Shelf) {
    s.shares.lock().unwrap().insert(
        "home".to_owned(),
        stored(
            "home",
            json!({
                "name": "home",
                "protocol": "nfs",
                "size_gib": 20,
                "access": [
                    {"cidr": "10.0.0.0/24", "mode": "write"},
                    {"cidr": "10.9.0.0/16", "mode": "read"},
                    {"principal": "alice", "mode": "read"}
                ]
            }),
            4,
        ),
    );
}

#[tokio::test]
async fn share_list_prints_protocol_size_and_access() {
    let (url, s) = serve().await;
    seed(&s);
    s.shares.lock().unwrap().insert(
        "media".to_owned(),
        stored(
            "media",
            json!({"name": "media", "protocol": "cephfs", "size_gib": 500}),
            1,
        ),
    );
    let r = pi(&url, "", &["share", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", r.out);
    for column in [
        "NAME", "ID", "PHASE", "PROTOCOL", "SIZE-GIB", "ACCESS", "AGE",
    ] {
        assert!(lines[0].contains(column), "{}", r.out);
    }
    let home = lines.iter().find(|l| l.contains("home")).expect("home row");
    assert!(home.contains("nfs"), "{home}");
    assert!(home.contains("20"), "{home}");
    assert!(home.contains("10.0.0.0/24=rw"), "{home}");
    assert!(home.contains("10.9.0.0/16=ro"), "{home}");
    // A rule naming a principal rather than a CIDR still shows up.
    assert!(home.contains("alice"), "{home}");
    let media = lines
        .iter()
        .find(|l| l.contains("media"))
        .expect("media row");
    assert!(media.contains("cephfs"), "{media}");

    // wide adds the client count and the message
    let r = pi(&url, "", &["share", "list", "-o", "wide", "--no-headers"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let home = r.out.lines().find(|l| l.contains("home")).unwrap();
    assert!(home.contains(ID), "{home}");
    assert!(home.contains("this build has no real"), "{home}");
    assert_eq!(s.queries.lock().unwrap()[0], "");
}

#[tokio::test]
async fn share_list_filters_by_label_and_project() {
    let (url, s) = serve().await;
    let r = pi(&url, "", &["share", "list", "--all", "-l", "tier=hot"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(s.queries.lock().unwrap()[0], "all=true&labels=tier=hot");
}

#[tokio::test]
async fn share_show_takes_a_name_or_an_id() {
    let (url, s) = serve().await;
    seed(&s);
    for name in ["home", ID] {
        let r = pi(&url, "", &["share", "show", name, "-o", "json"]).await;
        assert_eq!(r.code, 0, "{r:?}");
        let v: Value = serde_json::from_str(&r.out).unwrap();
        assert_eq!(v["name"], "home");
        assert_eq!(v["spec"]["protocol"], "nfs");
    }
    // Table output is the whole record as fields.
    let r = pi(&url, "", &["share", "show", "home"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("FIELD"), "{}", r.out);
    assert!(r.out.contains("spec.protocol"), "{}", r.out);

    let r = pi(&url, "", &["share", "show", "nope"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn share_create_sends_the_spec_the_server_wants() {
    let (url, s) = serve().await;
    let r = pi(
        &url,
        "",
        &[
            "share",
            "create",
            "home",
            "--protocol",
            "NFS",
            "--size",
            "25",
            "--cidr",
            "10.0.0.0/24",
            "--cidr",
            "10.9.0.0/16",
            "--mode",
            "read",
            "--description",
            "home dirs",
            "--label",
            "tier=hot",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        r.out.trim(),
        "share \"home\" created (share-0000000a)",
        "{}",
        r.out
    );
    let stored = &s.shares.lock().unwrap()["home"];
    assert_eq!(
        stored["spec"],
        json!({
            "name": "home",
            "protocol": "nfs",
            "size_gib": 25,
            "access": [
                {"cidr": "10.0.0.0/24", "mode": "read"},
                {"cidr": "10.9.0.0/16", "mode": "read"}
            ]
        })
    );
    assert_eq!(stored["description"], "home dirs");
    assert_eq!(stored["labels"]["tier"], "hot");
}

#[tokio::test]
async fn share_create_defaults_to_a_small_nfs_share() {
    let (url, s) = serve().await;
    let r = pi(&url, "", &["share", "create", "scratch"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let stored = &s.shares.lock().unwrap()["scratch"];
    assert_eq!(stored["spec"]["protocol"], "nfs");
    assert_eq!(stored["spec"]["size_gib"], 10);
    assert_eq!(stored["spec"]["access"], json!([]));
}

#[tokio::test]
async fn share_create_refuses_nonsense_before_asking_the_server() {
    let (url, s) = serve().await;
    for bad in ["10.0.0.0", "10.0.0.0/33", "host.example/24"] {
        let r = pi(&url, "", &["share", "create", "home", "--cidr", bad]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert!(r.err.contains("is not a CIDR"), "{}", r.err);
    }
    let r = pi(
        &url,
        "",
        &[
            "share",
            "create",
            "home",
            "--cidr",
            "10.0.0.0/24",
            "--mode",
            "maybe",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("mode must be read or write"), "{}", r.err);
    assert!(s.shares.lock().unwrap().is_empty());
}

#[tokio::test]
async fn share_delete_asks_unless_told_not_to() {
    let (url, s) = serve().await;
    seed(&s);
    // No terminal to confirm on, so this refuses rather than deleting.
    let r = pi(&url, "", &["share", "delete", "home"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert!(s.shares.lock().unwrap().contains_key("home"));

    let r = pi(&url, "", &["share", "delete", "home", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "share \"home\" deleted", "{}", r.out);
    assert!(s.shares.lock().unwrap().is_empty());
    assert_eq!(s.deletes.lock().unwrap()[0], "home");
}

#[tokio::test]
async fn share_access_lists_only_the_cidr_rules() {
    let (url, s) = serve().await;
    seed(&s);
    let r = pi(&url, "", &["share", "access", "list", "home"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<&str> = r.out.lines().collect();
    // The principal rule belongs to a different mechanism and is not shown.
    assert_eq!(lines.len(), 3, "{}", r.out);
    assert!(
        lines[0].contains("CIDR") && lines[0].contains("MODE"),
        "{}",
        r.out
    );
    assert!(
        lines[1].starts_with("10.0.0.0/24") && lines[1].contains("write"),
        "{r:?}"
    );
    assert!(
        lines[2].starts_with("10.9.0.0/16") && lines[2].contains("read"),
        "{r:?}"
    );

    let r = pi(&url, "", &["share", "access", "list", "home", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let rules: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(
        rules,
        json!([
            {"cidr": "10.0.0.0/24", "mode": "write"},
            {"cidr": "10.9.0.0/16", "mode": "read"}
        ])
    );
}

#[tokio::test]
async fn share_access_allow_sends_the_whole_spec_at_the_current_revision() {
    let (url, s) = serve().await;
    seed(&s);
    let r = pi(
        &url,
        "",
        &[
            "share",
            "access",
            "allow",
            "home",
            "--cidr",
            "192.168.1.0/24",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        r.out.trim(),
        "access share \"home\" allowed for 192.168.1.0/24",
        "{}",
        r.out
    );
    let (id, if_match, body) = &s.puts.lock().unwrap()[0];
    assert_eq!(id, ID, "the edit is addressed by id");
    assert_eq!(
        if_match.as_deref(),
        Some("4"),
        "If-Match is the revision read"
    );
    // Anything the CLI does not manage survives the round trip.
    assert_eq!(body["spec"]["name"], "home");
    assert_eq!(body["spec"]["protocol"], "nfs");
    assert_eq!(body["spec"]["size_gib"], 20);
    assert_eq!(
        body["spec"]["access"],
        json!([
            {"cidr": "10.0.0.0/24", "mode": "write"},
            {"cidr": "10.9.0.0/16", "mode": "read"},
            {"principal": "alice", "mode": "read"},
            {"cidr": "192.168.1.0/24", "mode": "write"}
        ])
    );
}

#[tokio::test]
async fn share_access_allow_replaces_the_mode_of_a_cidr_it_already_knows() {
    let (url, s) = serve().await;
    seed(&s);
    let r = pi(
        &url,
        "",
        &[
            "share",
            "access",
            "allow",
            "home",
            "--cidr",
            "10.9.0.0/16",
            "--mode",
            "write",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let access = s.puts.lock().unwrap()[0].2["spec"]["access"].clone();
    assert_eq!(
        access,
        json!([
            {"cidr": "10.0.0.0/24", "mode": "write"},
            {"cidr": "10.9.0.0/16", "mode": "write"},
            {"principal": "alice", "mode": "read"}
        ])
    );
    assert_eq!(access.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn share_access_allow_checks_the_mode_before_writing() {
    let (url, s) = serve().await;
    seed(&s);
    let r = pi(
        &url,
        "",
        &[
            "share",
            "access",
            "allow",
            "home",
            "--cidr",
            "10.2.0.0/16",
            "--mode",
            "maybe",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("mode must be read or write"), "{}", r.err);
    assert!(s.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn share_access_deny_drops_one_rule() {
    let (url, s) = serve().await;
    seed(&s);
    let r = pi(
        &url,
        "",
        &["share", "access", "deny", "home", "--cidr", "10.0.0.0/24"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        r.out.trim(),
        "access share \"home\" denied to 10.0.0.0/24",
        "{}",
        r.out
    );
    let access = s.puts.lock().unwrap()[0].2["spec"]["access"].clone();
    assert_eq!(
        access,
        json!([
            {"cidr": "10.9.0.0/16", "mode": "read"},
            {"principal": "alice", "mode": "read"}
        ])
    );
}

#[tokio::test]
async fn share_access_deny_refuses_a_rule_that_is_not_there() {
    let (url, s) = serve().await;
    seed(&s);
    let r = pi(
        &url,
        "",
        &["share", "access", "deny", "home", "--cidr", "172.16.0.0/12"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.err.contains("has no rule for '172.16.0.0/12'"),
        "{}",
        r.err
    );
    assert!(s.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_deny_names_only_the_cidr() {
    let (url, s) = serve().await;
    seed(&s);
    // A mode says nothing about which rule to drop, so deny does not take one.
    let r = pi(
        &url,
        "",
        &[
            "share",
            "access",
            "deny",
            "home",
            "--cidr",
            "10.0.0.0/24",
            "--mode",
            "read",
        ],
    )
    .await;
    assert_eq!(r.code, 2, "{r:?}");
    assert!(r.err.contains("unexpected argument '--mode'"), "{}", r.err);
    assert!(s.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_share_moved_underneath_an_access_edit_is_not_overwritten() {
    let (url, s) = serve_in(true).await;
    seed(&s);
    let r = pi(
        &url,
        "",
        &["share", "access", "allow", "home", "--cidr", "10.7.0.0/16"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("revision mismatch"), "{}", r.err);
    // The other writer's share stands; the rule the CLI wanted is not on it.
    let home = &s.shares.lock().unwrap()["home"];
    // The share was never touched: the other writer's size and the access
    // list the CLI meant to edit are both still as that writer left them.
    assert_eq!(home["spec"]["size_gib"], 20);
    assert_eq!(home["revision"], 4);
}

#[tokio::test]
async fn share_access_on_a_missing_share_reports_the_share() {
    let (url, _s) = serve().await;
    let r = pi(&url, "", &["share", "access", "list", "ghost"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("share ghost not found"), "{}", r.err);
    for verb in ["allow", "deny"] {
        let r = pi(
            &url,
            "",
            &["share", "access", verb, "ghost", "--cidr", "10.0.0.0/24"],
        )
        .await;
        assert_eq!(r.code, 1, "{r:?}");
        assert!(r.err.contains("share ghost not found"), "{}", r.err);
    }
}

#[tokio::test]
async fn apply_configures_a_share_and_its_access() {
    let (url, s) = serve().await;
    seed(&s);
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("share.yaml");
    std::fs::write(
        &manifest,
        "kind: share\nname: home\nspec:\n  size_gib: 30\n",
    )
    .unwrap();
    let r = pi(&url, "", &["apply", "-f", manifest.to_str().unwrap()]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("configured"), "{}", r.out);
    let home = &s.shares.lock().unwrap()["home"];
    assert_eq!(home["spec"]["size_gib"], 30);
    // A partial manifest leaves the rules alone.
    assert_eq!(home["spec"]["access"].as_array().unwrap().len(), 3);
}
