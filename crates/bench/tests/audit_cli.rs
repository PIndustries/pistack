// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi audit list` against a small in-process server that keeps the query
//! it was sent and answers with two records.

use axum::{
    extract::{RawQuery, State},
    routing::get,
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

type Queries = Arc<Mutex<Vec<String>>>;

async fn audit(State(seen): State<Queries>, RawQuery(query): RawQuery) -> Json<Value> {
    seen.lock().unwrap().push(query.unwrap_or_default());
    Json(json!({
        "items": [
            {
                "id": "audit-0199a5e0-0000-7000-8000-000000000002",
                "created_at": "2026-11-12T08:00:02Z",
                "spec": {
                    "method": "POST", "path": "/compute/hosts/rack1/drain",
                    "action": "drain", "kind": "hosts", "resource": "rack1",
                    "principal": "admin", "client": "10.0.0.9", "status": 200
                }
            },
            {
                "id": "audit-0199a5e0-0000-7000-8000-000000000001",
                "created_at": "2026-11-12T08:00:01Z",
                "spec": {
                    "method": "POST", "path": "/identity/tokens",
                    "action": "login", "kind": "tokens", "status": 401
                }
            }
        ],
        "next": "audit-0199a5e0-0000-7000-8000-000000000001",
        "total": 5
    }))
}

async fn serve() -> (String, Queries) {
    let seen = Queries::default();
    let app = Router::new()
        .route("/identity/audit", get(audit))
        .with_state(seen.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), seen)
}

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

#[tokio::test]
async fn audit_list_prints_a_table_and_says_when_there_is_more() {
    let (url, seen) = serve().await;
    let r = pi(&url, &["audit", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let lines: Vec<&str> = r.out.lines().collect();
    assert_eq!(lines.len(), 3, "{}", r.out);
    for column in [
        "WHEN", "WHO", "FROM", "ACTION", "KIND", "RESOURCE", "STATUS",
    ] {
        assert!(lines[0].contains(column), "{}", r.out);
    }
    for field in ["admin", "10.0.0.9", "drain", "hosts", "rack1", "200"] {
        assert!(lines[1].contains(field), "{}", r.out);
    }
    // A login nobody was named in shows a dash for who.
    assert!(lines[2].contains(" - "), "{}", r.out);
    assert!(lines[2].contains("401"), "{}", r.out);
    assert!(r.err.contains("--after audit-"), "{}", r.err);
    // Newest first, a hundred at a time, unless asked otherwise.
    assert_eq!(seen.lock().unwrap()[0], "limit=100&order=desc");
}

#[tokio::test]
async fn audit_list_sends_every_filter() {
    let (url, seen) = serve().await;
    let r = pi(
        &url,
        &[
            "audit",
            "list",
            "--since",
            "2026-11-12T08:00:00+01:00",
            "--until",
            "2026-11-12T09:00:00Z",
            "--principal",
            "alice",
            "--in-project",
            "web",
            "--action",
            "update_quotas",
            "--kind",
            "projects",
            "--resource",
            "a b",
            "--outcome",
            "failure",
            "--limit",
            "5",
            "--after",
            "audit-1",
            "--oldest-first",
            "-o",
            "json",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        seen.lock().unwrap()[0],
        "limit=5&order=asc&since=2026-11-12T07%3A00%3A00%2B00%3A00\
         &until=2026-11-12T09%3A00%3A00%2B00%3A00&principal=alice&project=web\
         &action=update_quotas&kind=projects&resource=a%20b&outcome=failure&after=audit-1"
    );
    // JSON output is the page as the server sent it.
    let page: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(page["total"], 5);
    assert_eq!(page["items"][0]["spec"]["action"], "drain");
}

#[tokio::test]
async fn audit_list_takes_an_age_for_a_time_and_refuses_nonsense() {
    let (url, seen) = serve().await;
    let before = chrono::Utc::now() - chrono::Duration::hours(2);
    let r = pi(&url, &["audit", "list", "--since", "2h"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let query = seen.lock().unwrap()[0].clone();
    let since = query
        .split('&')
        .find_map(|pair| pair.strip_prefix("since="))
        .expect("a since")
        .replace("%3A", ":")
        .replace("%2B", "+");
    let since = chrono::DateTime::parse_from_rfc3339(&since).unwrap();
    let late = (since.with_timezone(&chrono::Utc) - before).num_seconds();
    assert!((0..60).contains(&late), "{since} is not two hours ago");

    let r = pi(&url, &["audit", "list", "--since", "yesterday"]).await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("is not a time"), "{}", r.err);
    assert_eq!(seen.lock().unwrap().len(), 1);
}
