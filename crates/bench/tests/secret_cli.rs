// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi secret` and `pi cert` against a small in-process copy of the parts of
//! the secrets service they touch. The store here holds one payload per
//! secret, which is what the service holds: a put replaces it and bumps the
//! version. Nothing but `secret get` is answered with a payload.

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use base64::{engine::general_purpose::STANDARD as B64, Engine};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const SEC_ID: &str = "sec-0199a5e0-aaaa-7000-8000-000000000001";
const CERT_ID: &str = "cert-0199a5e0-aaaa-7000-8000-000000000002";
const CA_ID: &str = "ca-0199a5e0-aaaa-7000-8000-000000000003";
const PROJ_ID: &str = "proj-0199a5e0-aaaa-7000-8000-000000000004";

#[derive(Clone, Default)]
struct Store {
    /// The secret record as the service returns it: metadata, never a payload.
    secret: Arc<Mutex<Option<Value>>>,
    /// The payload it holds and the version it is at.
    payload: Arc<Mutex<Option<(String, u64)>>>,
    cert: Arc<Mutex<Option<Value>>>,
    /// `(method, path, body)` of every write, in order.
    writes: Arc<Mutex<Vec<(String, String, Value)>>>,
    /// The project each list call named, in order.
    lists: Arc<Mutex<Vec<String>>>,
}

impl Store {
    fn write(&self, method: &str, path: String, body: &Value) {
        self.writes
            .lock()
            .unwrap()
            .push((method.to_owned(), path, body.clone()));
    }
    fn last(&self) -> (String, String, Value) {
        self.writes
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a write")
    }
    fn listed(&self, project: &Query<BTreeMap<String, String>>) {
        self.lists
            .lock()
            .unwrap()
            .push(project.0.get("project").cloned().unwrap_or_default());
    }
}

fn err(status: StatusCode, msg: String) -> Response {
    (
        status,
        Json(json!({"error": {"code": "error", "message": msg, "details": {}}})),
    )
        .into_response()
}

fn authed(h: &HeaderMap) -> bool {
    bearer(h) == Some("t")
}

fn bearer(h: &HeaderMap) -> Option<&str> {
    h.get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
}

/// `{id}` takes a name or an id, as it does on the real service.
fn hit(want: &str, name: &str, id: &str) -> bool {
    want == name || want == id
}

/// What the store holds now, to check a round trip without going back
/// through the command that put it there.
fn payload(s: &Store) -> (String, u64) {
    s.payload.lock().unwrap().clone().unwrap_or_default()
}

fn string_field(body: &Value, key: &str) -> Option<String> {
    body.get(key).and_then(Value::as_str).map(|s| s.to_owned())
}

// ---- secrets ------------------------------------------------------------------

async fn list_secrets(
    State(s): State<Store>,
    h: HeaderMap,
    q: Query<BTreeMap<String, String>>,
) -> Response {
    if !authed(&h) && bearer(&h) != Some("blank") {
        return err(StatusCode::UNAUTHORIZED, "no credentials".into());
    }
    s.listed(&q);
    // A token scoped to one project sees only that project's rows; the
    // service has no way to widen a list.
    if q.get("project").map(|p| p.as_str()) == Some("proj-other") {
        return Json(json!({"items": [], "total": 0})).into_response();
    }
    let items: Vec<Value> = s.secret.lock().unwrap().iter().cloned().collect();
    let total = items.len();
    Json(json!({"items": items, "total": total})).into_response()
}

async fn create_secret(State(s): State<Store>, Json(body): Json<Value>) -> Response {
    s.write("POST", "/secrets/secrets".into(), &body);
    let name = match string_field(&body, "name") {
        Some(n) if !n.is_empty() => n,
        _ => return err(StatusCode::BAD_REQUEST, "name is required".into()),
    };
    // The service refuses a payload of zero bytes; so does this.
    if body["payload"].as_str().unwrap_or("").is_empty() {
        return err(StatusCode::BAD_REQUEST, "payload must not be empty".into());
    }
    let mut secret = json!({
        "id": SEC_ID,
        "name": name,
        "project": PROJ_ID,
        "created_at": "2026-11-14T00:00:00Z",
        "updated_at": "2026-11-14T00:00:00Z",
        "revision": 1,
        "spec": {"kind": "opaque", "content_type": "application/octet-stream"},
        "status": {"phase": "ready", "version": 1},
    });
    // The spec and metadata fields are flat on this create, as the service
    // takes them, so the CLI's body is checked by reading it back.
    for field in ["kind", "content_type", "expires_at", "rotate_after_days"] {
        if let Some(v) = body.get(field) {
            if !v.is_null() {
                secret["spec"][field] = v.clone();
            }
        }
    }
    for field in ["labels", "description"] {
        if let Some(v) = body.get(field) {
            if !v.is_null() {
                secret[field] = v.clone();
            }
        }
    }
    *s.payload.lock().unwrap() = Some((body["payload"].as_str().unwrap().into(), 1));
    *s.secret.lock().unwrap() = Some(secret.clone());
    (StatusCode::CREATED, Json(secret)).into_response()
}

async fn show_secret(State(s): State<Store>, Path(id): Path<String>) -> Response {
    let guard = s.secret.lock().unwrap();
    let Some(secret) = guard.as_ref() else {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    };
    if !hit(&id, "db-password", SEC_ID) {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    }
    Json(secret.clone()).into_response()
}

async fn update_secret(
    State(s): State<Store>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    s.write("PUT", format!("/secrets/secrets/{id}"), &body);
    let mut guard = s.secret.lock().unwrap();
    let Some(secret) = guard.as_mut() else {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    };
    if !hit(&id, "db-password", SEC_ID) {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    }
    let want = h
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().trim_matches('"').to_owned())
        .unwrap_or_default();
    if secret["revision"]
        .as_u64()
        .map(|r| r.to_string())
        .as_deref()
        != Some(want.as_str())
    {
        return err(StatusCode::CONFLICT, format!("revision mismatch: {want}"));
    }
    for field in ["description", "labels", "content_type"] {
        if let Some(v) = body.get(field) {
            if !v.is_null() {
                let slot = if field == "content_type" {
                    &mut secret["spec"]["content_type"]
                } else {
                    &mut secret[field]
                };
                *slot = v.clone();
            }
        }
    }
    secret["revision"] = json!(secret["revision"].as_u64().unwrap() + 1);
    Json(secret.clone()).into_response()
}

async fn delete_secret(State(s): State<Store>, Path(id): Path<String>) -> Response {
    s.write("DELETE", format!("/secrets/secrets/{id}"), &Value::Null);
    if !hit(&id, "db-password", SEC_ID) || s.secret.lock().unwrap().is_none() {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn get_payload(State(s): State<Store>, Path(id): Path<String>) -> Response {
    let (payload, version) = match s.payload.lock().unwrap().as_ref() {
        Some(p) => p.clone(),
        None => return err(StatusCode::NOT_FOUND, format!("secret {id} not found")),
    };
    if !hit(&id, "db-password", SEC_ID) {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    }
    let secret = s.secret.lock().unwrap().clone().unwrap_or(Value::Null);
    let content_type = secret["spec"]["content_type"].clone();
    let last_rotated = secret["status"]["last_rotated"].clone();
    Json(json!({
        "id": SEC_ID,
        "version": version,
        "content_type": content_type,
        "encoding": "base64",
        "payload": payload,
        "last_rotated": last_rotated,
    }))
    .into_response()
}

async fn put_payload(
    State(s): State<Store>,
    Path(id): Path<String>,
    h: HeaderMap,
    Json(body): Json<Value>,
) -> Response {
    s.write("PUT", format!("/secrets/secrets/{id}/payload"), &body);
    if !hit(&id, "db-password", SEC_ID) {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    }
    let payload = body["payload"].as_str().unwrap_or("");
    if payload.is_empty() {
        return err(StatusCode::BAD_REQUEST, "payload must not be empty".into());
    }
    // A rotation is a conditional write: the service wants the revision the
    // caller read, and refuses a payload written against a stale one.
    let want = h
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim().trim_matches('"').to_owned())
        .unwrap_or_default();
    let guard = s.secret.lock().unwrap();
    let Some(secret) = guard.as_ref() else {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    };
    if secret["revision"]
        .as_u64()
        .map(|r| r.to_string())
        .as_deref()
        != Some(want.as_str())
    {
        return err(StatusCode::CONFLICT, format!("revision mismatch: {want}"));
    }
    drop(guard);
    let mut cur = s.payload.lock().unwrap();
    let Some(slot) = cur.as_mut() else {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    };
    slot.0 = payload.to_owned();
    slot.1 += 1;
    let mut guard = s.secret.lock().unwrap();
    let Some(secret) = guard.as_mut() else {
        return err(StatusCode::NOT_FOUND, format!("secret {id} not found"));
    };
    secret["status"]["version"] = json!(slot.1);
    secret["revision"] = json!(secret["revision"].as_u64().unwrap() + 1);
    Json(secret.clone()).into_response()
}

// ---- certificates -------------------------------------------------------------

async fn list_certs(State(s): State<Store>, q: Query<BTreeMap<String, String>>) -> Response {
    s.listed(&q);
    let items: Vec<Value> = s.cert.lock().unwrap().iter().cloned().collect();
    let total = items.len();
    Json(json!({"items": items, "total": total})).into_response()
}

async fn issue_cert(State(s): State<Store>, Json(body): Json<Value>) -> Response {
    s.write("POST", "/secrets/certificates".into(), &body);
    let name = match string_field(&body, "name") {
        Some(n) if !n.is_empty() => n,
        _ => return err(StatusCode::BAD_REQUEST, "name is required".into()),
    };
    let spec = &body["spec"];
    if spec["common_name"].as_str().unwrap_or("").is_empty() {
        return err(StatusCode::BAD_REQUEST, "common_name is required".into());
    }
    if !spec["sans"].is_array() {
        return err(StatusCode::BAD_REQUEST, "sans must be a list".into());
    }
    match spec.get("issuer") {
        None | Some(Value::Null) => {}
        Some(v) if v.as_str() == Some(CA_ID) => {}
        Some(v) => return err(StatusCode::NOT_FOUND, format!("ca {v} not found")),
    }
    let cert = json!({
        "id": CERT_ID,
        "name": name,
        "project": PROJ_ID,
        "created_at": "2026-11-14T00:00:00Z",
        "updated_at": "2026-11-14T00:00:00Z",
        "revision": 1,
        "labels": body["labels"],
        "spec": spec,
        "status": {
            "phase": "ready",
            "serial": 4660,
            "not_before": "2026-11-14T00:00:00Z",
            "not_after": "2027-11-14T00:00:00Z",
            "fingerprint_sha256": "ab12",
            "cert_pem": "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
            "issuer": CA_ID,
            "key_secret": SEC_ID,
            "revoked": false,
        },
    });
    *s.cert.lock().unwrap() = Some(cert.clone());
    (StatusCode::CREATED, Json(cert)).into_response()
}

async fn show_cert(State(s): State<Store>, Path(id): Path<String>) -> Response {
    let guard = s.cert.lock().unwrap();
    let Some(cert) = guard.as_ref() else {
        return err(StatusCode::NOT_FOUND, format!("certificate {id} not found"));
    };
    if !hit(&id, "web", CERT_ID) {
        return err(StatusCode::NOT_FOUND, format!("certificate {id} not found"));
    }
    Json(cert.clone()).into_response()
}

async fn delete_cert(State(s): State<Store>, Path(id): Path<String>) -> Response {
    s.write(
        "DELETE",
        format!("/secrets/certificates/{id}"),
        &Value::Null,
    );
    if !hit(&id, "web", CERT_ID) {
        return err(StatusCode::NOT_FOUND, format!("certificate {id} not found"));
    }
    StatusCode::NO_CONTENT.into_response()
}

async fn cert_key(State(s): State<Store>, Path(id): Path<String>) -> Response {
    if !hit(&id, "web", CERT_ID) || s.cert.lock().unwrap().is_none() {
        return err(StatusCode::NOT_FOUND, format!("certificate {id} not found"));
    }
    Json(json!({
        "id": CERT_ID,
        "cert_pem": "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
        "key_pem": "-----BEGIN PRIVATE KEY-----\nKEYMATERIAL\n-----END PRIVATE KEY-----\n",
        "ca_pem": "-----BEGIN CERTIFICATE-----\nCA\n-----END CERTIFICATE-----\n",
    }))
    .into_response()
}

async fn revoke_cert(State(s): State<Store>, Path(id): Path<String>) -> Response {
    s.write(
        "POST",
        format!("/secrets/certificates/{id}/revoke"),
        &json!({}),
    );
    let mut guard = s.cert.lock().unwrap();
    let Some(cert) = guard.as_mut() else {
        return err(StatusCode::NOT_FOUND, format!("certificate {id} not found"));
    };
    if !hit(&id, "web", CERT_ID) {
        return err(StatusCode::NOT_FOUND, format!("certificate {id} not found"));
    }
    cert["status"]["revoked"] = json!(true);
    Json(cert.clone()).into_response()
}

async fn list_projects(State(s): State<Store>, q: Query<BTreeMap<String, String>>) -> Response {
    s.listed(&q);
    Json(json!({"items": [{"id": PROJ_ID, "name": "main"}], "total": 1})).into_response()
}

/// The projects the token can switch into, as identity answers it.
async fn my_projects(h: HeaderMap) -> Response {
    if !authed(&h) {
        return err(StatusCode::UNAUTHORIZED, "no credentials".into());
    }
    // `blank` is a token scoped to no project and bound in none: what an
    // unscoped admin's answer looks like from the CLI's side.
    if bearer(&h) == Some("blank") {
        return Json(json!([])).into_response();
    }
    Json(json!([
        {"id": PROJ_ID, "name": "main", "roles": ["admin"], "current": true},
        {"id": "proj-other", "name": "other", "roles": ["admin"], "current": false}
    ]))
    .into_response()
}

/// The record every test starts from: one secret at version 3, one issued
/// certificate, both in project `main`.
fn seeded() -> Store {
    let s = Store::default();
    *s.secret.lock().unwrap() = Some(json!({
        "id": SEC_ID,
        "name": "db-password",
        "project": PROJ_ID,
        "created_at": "2026-11-14T00:00:00Z",
        "updated_at": "2026-11-14T00:00:00Z",
        "revision": 2,
        "labels": {"tier": "db"},
        "spec": {"kind": "password", "content_type": "text/plain", "rotate_after_days": 90},
        "status": {"phase": "ready", "version": 3, "last_rotated": "2026-11-01T00:00:00Z"},
    }));
    *s.payload.lock().unwrap() = Some((B64.encode(b"hunter2"), 3));
    *s.cert.lock().unwrap() = Some(json!({
        "id": CERT_ID,
        "name": "web",
        "project": PROJ_ID,
        "created_at": "2026-11-14T00:00:00Z",
        "updated_at": "2026-11-14T00:00:00Z",
        "revision": 1,
        "spec": {
            "common_name": "web.example",
            "sans": ["web.example", "10.0.0.5"],
            "usage": "server",
            "ttl_days": 365,
        },
        "status": {
            "phase": "ready",
            "serial": 4660,
            "not_before": "2026-11-14T00:00:00Z",
            "not_after": "2027-11-14T00:00:00Z",
            "fingerprint_sha256": "ab12",
            "cert_pem": "-----BEGIN CERTIFICATE-----\nAAAA\n-----END CERTIFICATE-----\n",
            "issuer": CA_ID,
            "key_secret": SEC_ID,
            "revoked": false,
        },
    }));
    s
}

async fn serve() -> (String, Store) {
    let store = seeded();
    let app = Router::new()
        .route("/secrets/secrets", get(list_secrets).post(create_secret))
        .route(
            "/secrets/secrets/{id}",
            get(show_secret).put(update_secret).delete(delete_secret),
        )
        .route(
            "/secrets/secrets/{id}/payload",
            get(get_payload).put(put_payload),
        )
        .route("/secrets/certificates", get(list_certs).post(issue_cert))
        .route(
            "/secrets/certificates/{id}",
            get(show_cert).delete(delete_cert),
        )
        .route("/secrets/certificates/{id}/key", get(cert_key))
        .route("/secrets/certificates/{id}/revoke", post(revoke_cert))
        .route("/identity/projects", get(list_projects))
        .route("/identity/tokens/self/projects", get(my_projects))
        .with_state(store.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), store)
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(url: &str, stdin: &str, args: &[&str]) -> Run {
    pi_as(url, "t", stdin, args).await
}

async fn pi_as(url: &str, token: &str, stdin: &str, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let mut argv: Vec<String> = [
        "pi",
        "--config",
        dir.path().join("config.toml").to_str().unwrap(),
        "--url",
        url,
        "--token",
        token,
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
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
async fn secret_list_and_show_never_carry_a_payload() {
    let (url, store) = serve().await;
    let r = pi(&url, "", &["secret", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let mut lines = r.out.lines();
    let header = lines.next().unwrap();
    for want in ["NAME", "KIND", "VERSION", "ROTATED", "AGE"] {
        assert!(header.contains(want), "{header}");
    }
    let row = lines.next().unwrap();
    assert!(row.starts_with("db-password"), "{row}");
    assert!(row.contains("password"), "{row}");
    assert!(row.contains("3"), "{row}");
    assert!(!r.out.contains("hunter2"), "the payload leaked: {r:?}");

    let r = pi(&url, "", &["secret", "list", "-o", "wide"]).await;
    assert!(r.out.contains("CONTENT-TYPE"), "{r:?}");
    assert!(r.out.contains("text/plain"), "{r:?}");
    let r = pi(&url, "", &["secret", "list", "-o", "name"]).await;
    assert_eq!(r.out, "db-password\n");

    let r = pi(&url, "", &["secret", "show", "db-password"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("spec.kind"), "{r:?}");
    assert!(!r.out.contains("hunter2"), "{r:?}");

    let r = pi(&url, "", &["secret", "show", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
    assert!(store.writes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn secret_get_prints_the_value_and_nothing_else() {
    let (url, _store) = serve().await;
    let r = pi(&url, "", &["secret", "get", "db-password"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "hunter2\n");
    // Which version is coming out is said where it will not be redirected
    // into the file the value is being piped to.
    assert!(r.err.contains("version 3"), "{r:?}");
    assert!(r.err.contains("2026-11-01"), "{r:?}");

    // A piped value is the value whatever -o says.
    for fmt in ["json", "yaml", "name", "wide"] {
        let r = pi(&url, "", &["-o", fmt, "secret", "get", "db-password"]).await;
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(r.out, "hunter2\n", "-o {fmt} {r:?}");
    }

    let r = pi(&url, "", &["secret", "get", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
    assert_eq!(r.out, "");
}

#[tokio::test]
async fn secret_get_refuses_a_version_it_does_not_hold() {
    let (url, _store) = serve().await;
    let r = pi(
        &url,
        "",
        &["secret", "get", "db-password", "--version", "3"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "hunter2\n");

    let r = pi(
        &url,
        "",
        &["secret", "get", "db-password", "--version", "2"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("version 3"), "{r:?}");
    assert!(r.err.contains("version 2 is not kept"), "{r:?}");
    assert_eq!(r.out, "", "a refused version prints nothing");
}

#[tokio::test]
async fn secret_create_reads_the_value_from_a_file_or_stdin() {
    let (url, store) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("value");

    let r = pi(
        &url,
        "",
        &["secret", "create", "fresh", "--value-file", "-"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("empty"), "{r:?}");
    assert!(store.writes.lock().unwrap().is_empty());

    std::fs::write(&file, "from a file\n\n").unwrap();
    let r = pi(
        &url,
        "",
        &[
            "secret",
            "create",
            "api-token",
            "--value-file",
            file.to_str().unwrap(),
            "--kind",
            "token",
            "--content-type",
            "text/plain",
            "--label",
            "tier=db",
            "--label",
            "env=prod",
            "--description",
            "for the api",
            "--rotate-after-days",
            "30",
            "--expires-at",
            "2027-01-01T00:00:00Z",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.out.starts_with("secret \"api-token\" created (sec-"),
        "{r:?}"
    );
    let (method, path, body) = store.last();
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("POST", "/secrets/secrets")
    );
    // keep takes all of this flat, beside the payload.
    assert_eq!(body["name"], "api-token");
    assert_eq!(body["kind"], "token");
    assert_eq!(body["content_type"], "text/plain");
    assert_eq!(body["labels"], json!({"tier": "db", "env": "prod"}));
    assert_eq!(body["description"], "for the api");
    assert_eq!(body["rotate_after_days"], 30);
    assert_eq!(body["expires_at"], "2027-01-01T00:00:00Z");
    // A trailing newline is how a file ends, not part of the secret.
    assert_eq!(body["payload"], B64.encode(b"from a file"));
    assert_eq!(body["encoding"], "base64");
    assert!(body.get("spec").is_none(), "{body}");
    // The payload really was made from what was read.
    assert_eq!(payload(&store), (B64.encode(b"from a file"), 1));

    // Stdin is the default and needs no flag.
    let r = pi(&url, "piped away\n", &["secret", "create", "s2"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(store.last().2["payload"], B64.encode(b"piped away"));
    assert_eq!(store.last().2["kind"], "opaque");

    // `@file` is taken as a file too.
    std::fs::write(&file, "again").unwrap();
    let at = format!("@{}", file.to_str().unwrap());
    let r = pi(&url, "", &["secret", "create", "s3", "--value-file", &at]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(store.last().2["payload"], B64.encode(b"again"));

    // Empty is empty however it arrives, and a missing file is named.
    let r = pi(&url, "\n", &["secret", "create", "s4"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("empty"), "{r:?}");
    let r = pi(
        &url,
        "",
        &["secret", "create", "s5", "--value-file", "missing.txt"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("missing.txt"), "{r:?}");
}

#[tokio::test]
async fn secret_put_replaces_the_value_and_bumps_the_version() {
    let (url, store) = serve().await;
    let r = pi(&url, "rotated in\n", &["secret", "put", "db-password"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.out.starts_with("secret \"db-password\" put (sec-"),
        "{r:?}"
    );
    let (method, path, body) = store.last();
    // `{id}` takes a name, so a name is sent to a name; a payload only ever
    // travels once the name has been found.
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("PUT", format!("/secrets/secrets/{SEC_ID}/payload").as_str())
    );
    assert_eq!(
        body,
        json!({"payload": B64.encode(b"rotated in"), "encoding": "base64"})
    );

    assert_eq!(payload(&store), (B64.encode(b"rotated in"), 4));
    let r = pi(&url, "", &["secret", "get", "db-password"]).await;
    assert_eq!(r.out, "rotated in\n");
    assert!(r.err.contains("version 4"), "{r:?}");

    let before = store.writes.lock().unwrap().len();
    let r = pi(&url, "x", &["secret", "put", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
    assert_eq!(
        store.writes.lock().unwrap().len(),
        before,
        "a put that was refused still sent a payload"
    );
}

#[tokio::test]
async fn secret_delete_needs_yes() {
    let (url, store) = serve().await;
    let r = pi(&url, "", &["secret", "delete", "db-password"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("--yes"), "{r:?}");
    assert!(store.writes.lock().unwrap().is_empty());

    let r = pi(&url, "", &["secret", "delete", "db-password", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "secret \"db-password\" deleted\n");
    let (method, path, _) = store.last();
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("DELETE", "/secrets/secrets/db-password")
    );

    let r = pi(&url, "", &["secret", "delete", "nope", "-y"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
}

#[tokio::test]
async fn secret_list_all_asks_for_every_project() {
    let (url, store) = serve().await;
    let r = pi(&url, "", &["secret", "list", "--all"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("db-password"), "{r:?}");
    // One request per project the token can work in; a row the token sees from
    // two of them is still listed once.
    let lists = store.lists.lock().unwrap().clone();
    assert_eq!(
        lists,
        vec![PROJ_ID.to_owned(), "proj-other".to_owned()],
        "{lists:?}"
    );
    assert_eq!(r.out.matches("db-password").count(), 1, "{r:?}");

    // A token that can work in no project at all still gets its own scope.
    let r = pi_as(&url, "blank", "", &["secret", "list", "--all"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("db-password"), "{r:?}");
}

#[tokio::test]
async fn cert_list_and_show_read_the_record() {
    let (url, _store) = serve().await;
    let r = pi(&url, "", &["cert", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let mut lines = r.out.lines();
    let header = lines.next().unwrap();
    for want in [
        "NAME", "SUBJECT", "SANS", "USAGE", "SERIAL", "REVOKED", "EXPIRES",
    ] {
        assert!(header.contains(want), "{header}");
    }
    let row = lines.next().unwrap();
    assert!(row.starts_with("web "), "{row}");
    assert!(row.contains("web.example"), "{row}");
    assert!(row.contains("web.example,10.0.0.5"), "{row}");
    // 4660 is shown in hex.
    assert!(row.contains("1234"), "{row}");
    assert!(row.contains("false"), "{row}");

    let r = pi(&url, "", &["cert", "list", "-o", "wide"]).await;
    assert!(r.out.contains("FINGERPRINT"), "{r:?}");
    assert!(r.out.contains("ab12"), "{r:?}");

    let r = pi(&url, "", &["cert", "show", "web"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("BEGIN CERTIFICATE"), "{r:?}");

    let r = pi(&url, "", &["cert", "show", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
}

#[tokio::test]
async fn cert_issue_sends_the_spec_the_service_wants() {
    let (url, store) = serve().await;
    let r = pi(
        &url,
        "",
        &[
            "cert",
            "issue",
            "web",
            "--common-name",
            "web.example",
            "--san",
            "web.example",
            "--san",
            "10.0.0.5",
            "--usage",
            "client",
            "--ttl-days",
            "30",
            "--label",
            "env=prod",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.starts_with("cert \"web\" created (cert-"), "{r:?}");
    let (method, path, body) = store.last();
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("POST", "/secrets/certificates")
    );
    assert_eq!(body["name"], "web");
    assert_eq!(body["labels"], json!({"env": "prod"}));
    assert_eq!(
        body["spec"],
        json!({
            "common_name": "web.example",
            "sans": ["web.example", "10.0.0.5"],
            "usage": "client",
            "ttl_days": 30,
        })
    );

    // With no subject given the name is the subject, and the default usage is
    // a server's. Fields nobody named are absent, not empty.
    let r = pi(&url, "", &["cert", "issue", "other"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let body = store.last().2;
    assert_eq!(body["spec"]["common_name"], "other");
    assert_eq!(body["spec"]["usage"], "server");
    assert!(body["spec"].get("sans").is_some(), "{body}");
    assert_eq!(body["spec"]["sans"], json!([]));
    assert!(body["spec"].get("issuer").is_none(), "{body}");
    assert!(body["spec"].get("ttl_days").is_none(), "{body}");

    // An issuer is a CA id: there is no command yet to look one up by name.
    let r = pi(
        &url,
        "",
        &["cert", "issue", "web", "--issuer", "pistack-issuing"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("ca-"), "{r:?}");
    let r = pi(&url, "", &["cert", "issue", "web", "--issuer", CA_ID]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(store.last().2["spec"]["issuer"], CA_ID);
}

#[tokio::test]
async fn cert_key_prints_the_pem_bodies() {
    let (url, _store) = serve().await;
    let r = pi(&url, "", &["cert", "key", "web"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines[0], "----- certificate -----", "{r:?}");
    assert!(lines[1].starts_with("-----BEGIN CERTIFICATE"), "{r:?}");
    assert!(r.out.contains("----- private key -----"), "{r:?}");
    assert!(r.out.contains("KEYMATERIAL"), "{r:?}");
    assert!(r.out.contains("----- ca chain -----"), "{r:?}");

    let r = pi(&url, "", &["-o", "json", "cert", "key", "web"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert!(
        v["key_pem"].as_str().unwrap().contains("PRIVATE KEY"),
        "{r:?}"
    );

    let r = pi(&url, "", &["cert", "key", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
}

#[tokio::test]
async fn cert_revoke_keeps_the_record_and_says_revoked() {
    let (url, store) = serve().await;
    let r = pi(&url, "", &["cert", "revoke", "web"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.err
            .contains("refusing to revoke cert \"web\" without --yes"),
        "{r:?}"
    );
    assert!(store.writes.lock().unwrap().is_empty());

    let r = pi(&url, "", &["cert", "revoke", "web", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "cert \"web\" revoked\n");
    let (method, path, body) = store.last();
    assert_eq!(method, "POST");
    assert_eq!(path, format!("/secrets/certificates/{CERT_ID}/revoke"));
    assert_eq!(body, json!({}));

    // A revoked certificate is still listed, and marked as one.
    let r = pi(&url, "", &["cert", "list"]).await;
    assert!(r.out.contains("true"), "{r:?}");

    let r = pi(&url, "", &["cert", "revoke", "nope", "-y"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{r:?}");
}

#[tokio::test]
async fn cert_delete_takes_the_record_away() {
    let (url, store) = serve().await;
    let r = pi(&url, "", &["cert", "delete", "web"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("--yes"), "{r:?}");
    let r = pi(&url, "", &["cert", "delete", "web", "-y"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "cert \"web\" deleted\n");
    assert_eq!(store.last().1, "/secrets/certificates/web");
}

#[tokio::test]
async fn a_manifest_secret_without_a_payload_is_not_made() {
    let (url, store) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("s.yaml");

    // The record exists: apply configures it and sends no payload.
    std::fs::write(
        &manifest,
        "kind: secret\nname: db-password\ndescription: from a manifest\nlabels:\n  tier: db\n",
    )
    .unwrap();
    let r = pi(&url, "", &["apply", "-f", manifest.to_str().unwrap()]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "secret \"db-password\" configured\n");
    let (method, path, body) = store.last();
    assert_eq!(
        (method.as_str(), path.as_str()),
        ("PUT", format!("/secrets/secrets/{SEC_ID}").as_str())
    );
    assert_eq!(body["description"], "from a manifest");
    assert_eq!(body["labels"], json!({"tier": "db"}));
    assert!(body.get("payload").is_none(), "{body}");

    // Nothing of the sort exists: the caller is told what to run instead of
    // getting a record with an empty payload.
    std::fs::write(
        &manifest,
        "kind: secret\nname: fresh\nspec:\n  kind: password\n",
    )
    .unwrap();
    let r = pi(&url, "", &["apply", "-f", manifest.to_str().unwrap()]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out, "");
    assert!(r.err.contains("pi secret create fresh"), "{r:?}");
    assert!(
        store.writes.lock().unwrap().len() == 1,
        "{:?}",
        store.writes
    );
}

#[tokio::test]
async fn the_nouns_are_in_the_help_and_completion() {
    let (url, _store) = serve().await;
    let r = pi(&url, "", &["--help"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("secret"), "{r:?}");
    assert!(r.out.contains("cert"), "{r:?}");

    let r = pi(&url, "", &["secret", "--help"]).await;
    for verb in ["list", "show", "create", "delete", "get", "put"] {
        assert!(r.out.contains(verb), "secret --help: {r:?}");
    }
    let r = pi(&url, "", &["cert", "--help"]).await;
    for verb in ["list", "show", "issue", "delete", "revoke", "key"] {
        assert!(r.out.contains(verb), "cert --help: {r:?}");
    }
    // A value is never a command-line argument.
    let r = pi(&url, "", &["secret", "create", "--help"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("--value-file"), "{r:?}");
    assert!(!r.out.contains("--value <"), "{r:?}");

    let r = pi(&url, "", &["completion", "bash"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("secret"), "{r:?}");
}
