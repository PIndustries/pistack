// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi binding create` against a small in-process server that keeps the
//! bodies it was sent. The scope must be named on the command line:
//! `--in-project` for one project, `--system` for the whole cloud.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

type Bodies = Arc<Mutex<Vec<Value>>>;

async fn create(State(seen): State<Bodies>, Json(body): Json<Value>) -> Response {
    seen.lock().unwrap().push(body.clone());
    (
        StatusCode::CREATED,
        Json(json!({
            "id": "rb-0199a5e0-0000-7000-8000-000000000001",
            "spec": {
                "principal": body["principal"], "role": body["role"],
                "scope": {"type": if body["project"].is_null() { "system" } else { "project" }},
            }
        })),
    )
        .into_response()
}

async fn serve() -> (String, Bodies) {
    let seen = Bodies::default();
    let app = Router::new()
        .route("/identity/bindings", post(create))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(url: &str, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let mut argv: Vec<String> = ["pi", "--config", config.to_str().unwrap(), "--url", url]
        .iter()
        .map(|s| s.to_string())
        .collect();
    argv.extend(["--token".to_owned(), "t".to_owned()]);
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

const WHO: [&str; 6] = [
    "binding",
    "create",
    "--principal",
    "alice",
    "--role",
    "member",
];

#[tokio::test]
async fn a_binding_must_say_where_it_applies() {
    let (url, seen) = serve().await;
    let r = pi(&url, &WHO).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.err.contains(
            "say where the binding applies: --in-project PROJECT for one project, or --system for the whole cloud"
        ),
        "{}",
        r.err
    );
    assert!(seen.lock().unwrap().is_empty(), "no request was sent");
}

#[tokio::test]
async fn system_scope_sends_a_body_with_no_project() {
    let (url, seen) = serve().await;
    let r = pi(&url, &[&WHO[..], &["--system"]].concat()).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("binding"), "{}", r.out);
    let bodies = seen.lock().unwrap().clone();
    assert_eq!(
        bodies,
        [json!({"principal": "alice", "role": "member"})],
        "a system body carries no project"
    );
}

#[tokio::test]
async fn in_project_goes_into_the_body_as_the_project() {
    let (url, seen) = serve().await;
    let r = pi(&url, &[&WHO[..], &["--in-project", "web"]].concat()).await;
    assert_eq!(r.code, 0, "{r:?}");
    let bodies = seen.lock().unwrap().clone();
    assert_eq!(
        bodies,
        [json!({"principal": "alice", "role": "member", "project": "web"})]
    );
}

#[tokio::test]
async fn system_and_in_project_conflict() {
    let (url, seen) = serve().await;
    let r = pi(
        &url,
        &[&WHO[..], &["--system", "--in-project", "web"]].concat(),
    )
    .await;
    assert_eq!(r.code, 2, "{r:?}");
    assert!(r.err.contains("cannot be used with"), "{}", r.err);
    assert!(seen.lock().unwrap().is_empty(), "no request was sent");
}
