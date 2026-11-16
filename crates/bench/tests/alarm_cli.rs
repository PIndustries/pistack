// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi alarm` and `pi metric` driven end to end against the telemetry
//! service itself: the real `gauge` router on a local port, so the paths,
//! bodies and answers the CLI works against are the ones a server gives.

use axum::{
    extract::Request,
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json, Router,
};
use bench::cli::{self, Io};
use chrono::Utc;
use gauge::Gauge;
use gauge::Sample;
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::Arc;

const TOKEN: &str = "tok-alarm-1";

fn admin_caller() -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "cloud-admin".into(),
        project: None,
        roles: vec!["admin".into()],
        system: true,
    }
}

/// What the gateway does before a module sees a request: check the token and
/// say who is calling.
async fn gate(req: Request, next: axum::middleware::Next) -> axum::response::Response {
    if !bearer_ok(req.headers()) {
        return (
            StatusCode::UNAUTHORIZED,
            Json(json!({"error": {"code": "unauthorized", "message": "no credentials", "details": {}}})),
        )
            .into_response();
    }
    let (mut parts, body) = req.into_parts();
    parts.extensions.insert(admin_caller());
    next.run(Request::from_parts(parts, body)).await
}

fn bearer_ok(h: &HeaderMap) -> bool {
    h.get("authorization").and_then(|v| v.to_str().ok()) == Some(&format!("Bearer {TOKEN}"))
}

/// The gauge module behind a bearer-token check, which is what the gateway
/// puts in front of it. Each test gets a server of its own, so no series or
/// alarm is left over from another.
async fn serve() -> String {
    let gauge = Gauge;
    let store = Store::in_memory().unwrap();
    let ctx = ModuleContext {
        events: store.events(),
        store: Arc::new(store),
        shutdown: pistack_types::module::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    let app = Router::new()
        .nest("/gauge", gauge.router(ctx))
        .layer(axum::middleware::from_fn(gate));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
}

/// One request to the served module, to set up and to check what the CLI did.
async fn call(url: &str, method: &str, path: &str, body: Option<Value>) -> (StatusCode, Value) {
    let mut req = reqwest::Client::new()
        .request(
            reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
            format!("{url}{path}"),
        )
        .header("authorization", format!("Bearer {TOKEN}"));
    if let Some(v) = body {
        req = req.json(&v);
    }
    let resp = req.send().await.unwrap();
    let status = resp.status();
    let text = resp.text().await.unwrap();
    let v = if text.is_empty() {
        Value::Null
    } else {
        serde_json::from_str(&text).unwrap_or(Value::String(text))
    };
    (status, v)
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(url: &str, stdin: &str, tty: bool, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let config = dir.path().join("config.toml");
    let mut argv: Vec<String> = ["pi", "--config", config.to_str().unwrap(), "--url", url]
        .iter()
        .map(|s| s.to_string())
        .collect();
    argv.extend(["--token".to_owned(), TOKEN.to_owned()]);
    argv.extend(args.iter().map(|s| s.to_string()));
    let mut input = Cursor::new(stdin.as_bytes().to_vec());
    let mut out = Vec::new();
    let mut err = Vec::new();
    let code = {
        let mut io = Io {
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut err,
            tty,
        };
        cli::run(argv, &mut io).await.expect("run")
    };
    Run {
        code,
        out: String::from_utf8(out).unwrap(),
        err: String::from_utf8(err).unwrap(),
    }
}

async fn cli(url: &str, args: &[&str]) -> Run {
    pi(url, "", false, args).await
}

/// The window belongs to the process, and a sample a test leaves behind
/// re-evaluates every alarm in every test's store. One lock, held by
/// whoever is visiting a server, keeps those runs from stepping on each
/// other.
fn window() -> &'static std::sync::Mutex<()> {
    static WINDOW: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    WINDOW.get_or_init(|| std::sync::Mutex::new(()))
}

/// One test's whole visit to its server: points written, commands run,
/// results checked. Held against the other tests, so no sample arriving
/// from another test re-decides an alarm this one is reading. The guard
/// is meant to ride across the awaits below: that is what the visit is.
#[allow(clippy::await_holding_lock)]
async fn at_the_window<T>(f: impl std::future::Future<Output = T>) -> T {
    let held = window().lock().unwrap_or_else(|e| e.into_inner());
    let out = f.await;
    drop(held);
    out
}

/// The table as rows of cells: columns are two or more spaces apart.
fn rows(out: &str) -> Vec<Vec<String>> {
    out.lines()
        .map(|line| {
            line.split("  ")
                .map(str::trim)
                .filter(|cell| !cell.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .collect()
}

/// Post a sample. What a caller may post is a series of its own: a name that
/// starts with `pistack_` belongs to the software that measures it.
async fn post(url: &str, metric: &str, value: f64) {
    let (st, body) = call(
        url,
        "POST",
        "/gauge/samples",
        Some(json!({"metric": metric, "value": value})),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{body}");
}

/// A point placed in the window the way the software that measures places
/// its own: no route takes these, so they go straight in. `stamp` keeps the
/// points of one test in order with each other even when they land in the
/// same millisecond.
fn record(metric: &str, value: f64, stamp: chrono::Duration) {
    gauge::samples().push(Sample {
        metric: metric.to_owned(),
        value,
        timestamp: Utc::now() + stamp,
    });
}

/// The series a test posts, named after itself. The window belongs to the
/// process, so a point left by another test must not read as one of its
/// own.
fn mine(test: &str, part: Option<&str>) -> String {
    match part {
        Some(p) => format!("queue.{test}.{p}"),
        None => format!("queue.{test}"),
    }
}

/// The same series with a label, which makes it a different one.
fn hosted(test: &str, host: &str) -> String {
    format!("queue.{test}{{host=\"{host}\"}}")
}

/// Make an alarm through the API, under a name no other test uses.
async fn create(url: &str, name: &str, metric: &str, comparison: &str, threshold: f64) -> String {
    let (st, v) = call(
        url,
        "POST",
        "/gauge/alarms",
        Some(json!({
            "name": name,
            "spec": {
                "name": name,
                "metric": metric,
                "comparison": comparison,
                "threshold": threshold,
                "for_seconds": 0
            }
        })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED, "{v}");
    v["id"].as_str().unwrap().to_owned()
}

#[tokio::test]
async fn alarm_create_then_list_and_show() {
    at_the_window(async {
        let url = serve().await;
        record("queue.depth.create", 12.0, chrono::Duration::zero());

        let r = cli(
            &url,
            &[
                "alarm",
                "create",
                "alarm-cli-create",
                "--metric",
                "queue.depth.create",
                "--comparison",
                "gt",
                "--threshold",
                "10",
                "--description",
                "the queue is long",
                "--label",
                "team=core",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(
            r.out.contains("alarm \"alarm-cli-create\" created"),
            "{}",
            r.out
        );

        // What the server kept is what the flags asked for.
        let (st, v) = call(&url, "GET", "/gauge/alarms/alarm-cli-create", None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["spec"]["metric"], "queue.depth.create");
        assert_eq!(v["spec"]["comparison"], "gt");
        assert_eq!(v["spec"]["threshold"], 10.0);
        assert_eq!(v["spec"]["for_seconds"], 0);
        assert_eq!(v["description"], "the queue is long");
        assert_eq!(v["labels"]["team"], "core");
        // A sample is already over the threshold, so the alarm is born firing.
        assert_eq!(v["status"]["phase"], "Firing", "{v}");
        assert_eq!(v["status"]["last_value"], 12.0);

        let r = cli(&url, &["alarm", "list"]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        let table = rows(&r.out);
        assert_eq!(table[0][0], "NAME");
        assert!(table[0].contains(&"THRESHOLD".to_owned()), "{}", r.out);
        let row = table
            .iter()
            .find(|row| row[0] == "alarm-cli-create")
            .expect("the new alarm is listed");
        assert!(row.contains(&"Firing".to_owned()), "{row:?}");
        assert!(row.contains(&"gt".to_owned()), "{row:?}");

        let r = cli(&url, &["alarm", "ls", "-o", "json"]).await;
        let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
        let mine = items
            .iter()
            .filter(|it| it["name"] == "alarm-cli-create")
            .collect::<Vec<_>>();
        assert_eq!(mine.len(), 1);
        assert_eq!(mine[0]["spec"]["metric"], "queue.depth.create");

        let r = cli(&url, &["alarm", "ls", "-o", "name"]).await;
        assert!(r.out.lines().any(|l| l == "alarm-cli-create"), "{}", r.out);

        let r = cli(&url, &["alarm", "list", "-o", "yaml"]).await;
        let back: Vec<Value> = serde_yaml_ng::from_str(&r.out).unwrap();
        assert!(back.iter().any(|it| it["name"] == "alarm-cli-create"));

        // show by name, and by the id the create reported.
        let id = v["id"].as_str().unwrap();
        let r = cli(&url, &["alarm", "show", "alarm-cli-create"]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(r.out.contains("spec.metric"), "{}", r.out);
        assert!(r.out.contains("queue.depth.create"), "{}", r.out);
        let r = cli(&url, &["alarm", "show", id, "-o", "json"]).await;
        let shown: Value = serde_json::from_str(&r.out).unwrap();
        assert_eq!(shown["id"], id);

        // `show` gives one record as fields either way; a table of one is what
        // `list` makes.
        for fmt in ["wide"] {
            let r = cli(&url, &["alarm", "get", "alarm-cli-create", "-o", fmt]).await;
            assert_eq!(r.code, 0, "{}", r.err);
            assert!(r.out.contains("status.series"), "{}", r.out);
            assert!(r.out.contains("queue.depth.create"), "{}", r.out);
        }
        let r = cli(&url, &["alarm", "list", "-o", "wide"]).await;
        let table = rows(&r.out);
        assert!(table[0].contains(&"SERIES".to_owned()), "{}", r.out);
        assert!(r.out.contains("queue.depth.create"), "{}", r.out);

        let r = cli(&url, &["alarm", "show", "nope"]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert_eq!(r.err.trim(), "error: alarm 'nope' not found");
    })
    .await
}

#[tokio::test]
async fn alarm_create_takes_each_comparison_and_for_seconds() {
    at_the_window(async {
        let url = serve().await;
        record(
            "pistack_host_load1{host=\"cmp\"}",
            0.4,
            chrono::Duration::zero(),
        );

        for (name, comparison, threshold) in [
            ("lt-low", "lt", 1.0),
            ("eq-exact", "eq", 0.4),
            ("gt-high", "gt", 9.0),
        ] {
            let r = cli(
                &url,
                &[
                    "alarm",
                    "create",
                    name,
                    "--metric",
                    "pistack_host_load1{host=\"cmp\"}",
                    "--comparison",
                    comparison,
                    "--threshold",
                    &threshold.to_string(),
                ],
            )
            .await;
            assert_eq!(r.code, 0, "{name}: {}", r.err);
        }
        for (name, phase) in [
            ("lt-low", "Firing"),
            ("eq-exact", "Firing"),
            ("gt-high", "Ok"),
        ] {
            let (st, v) = call(&url, "GET", &format!("/gauge/alarms/{name}"), None).await;
            assert_eq!(st, StatusCode::OK, "{v}");
            assert_eq!(v["status"]["phase"], phase, "{name}");
            assert_eq!(v["spec"]["for_seconds"], 0);
            assert_eq!(v["status"]["series"], "pistack_host_load1{host=\"cmp\"}");
        }

        // A hold asks for a run of crossings long enough to span it. Points a
        // quarter of an hour apart, three hours deep, span a hold of five
        // minutes, so it fires from the window.
        for age in (0..12).rev() {
            record(
                "pistack_host_load1{host=\"held\"}",
                0.4,
                -chrono::Duration::minutes(15 * age),
            );
        }
        let r = cli(
            &url,
            &[
                "alarm",
                "create",
                "held-long",
                "--metric",
                "pistack_host_load1{host=\"held\"}",
                "--comparison",
                "lt",
                "--threshold",
                "1",
                "--for-seconds",
                "300",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{}", r.err);
        let (st, v) = call(&url, "GET", "/gauge/alarms/held-long", None).await;
        assert_eq!(st, StatusCode::OK, "{v}");
        assert_eq!(v["spec"]["for_seconds"], 300);
        assert_eq!(v["status"]["phase"], "Firing", "{v}");
        assert_eq!(v["status"]["series"], "pistack_host_load1{host=\"held\"}");

        let r = cli(
            &url,
            &[
                "alarm",
                "create",
                "bad",
                "--metric",
                "m",
                "--comparison",
                "ne",
                "--threshold",
                "1",
            ],
        )
        .await;
        assert_eq!(r.code, 2, "{r:?}");
        assert!(r.err.contains("possible values"), "{}", r.err);
    })
    .await
}

#[tokio::test]
async fn alarm_show_samples_prints_the_deciding_series() {
    at_the_window(async {
        let url = serve().await;
        for (i, v) in [4.0, 9.0, 12.0].iter().enumerate() {
            // A point placed by hand, so its stamp is known: the route and the
            // point it triggers can land in one millisecond, and the window
            // would then order them arbitrarily.
            record(
                "queue.depth.samples",
                *v,
                chrono::Duration::milliseconds(i as i64),
            );
        }
        create(&url, "alarm-cli-samples", "queue.depth.samples", "gt", 10.0).await;

        let r = cli(&url, &["alarm", "show", "alarm-cli-samples", "--samples"]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        // The record's own table, then the series that decided it and its points.
        assert!(r.out.contains("status.series"), "{}", r.out);
        let (record, points) = r
            .out
            .split_once("\nqueue.depth.samples\n")
            .expect("the deciding series is named");
        assert!(record.contains("spec.metric"), "{record}");
        let table = rows(points);
        assert_eq!(table[0], ["TIME", "VALUE"], "{}", r.out);
        // The three points held for the series, oldest first.
        let rows: Vec<Vec<String>> = table.iter().skip(1).cloned().collect();
        assert_eq!(rows.len(), 3, "{}", r.out);
        assert_eq!(rows[0].last().unwrap(), "4.0");
        assert_eq!(rows[2].last().unwrap(), "12.0");

        // -o json is the record with its points under `samples`, one document.
        let r = cli(
            &url,
            &[
                "alarm",
                "show",
                "alarm-cli-samples",
                "--samples",
                "-o",
                "json",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{r:?}");
        let shown: Value = serde_json::from_str(&r.out).expect("one json document");
        assert_eq!(shown["status"]["series"], "queue.depth.samples");
        let points = shown["samples"].as_array().expect("the points ride along");
        assert_eq!(points.len(), 3, "{shown}");
        assert_eq!(points[2]["value"], 12.0);
    })
    .await
}

#[tokio::test]
async fn alarm_update_changes_only_what_is_named() {
    at_the_window(async {
        let url = serve().await;
        // A point an hour old spans nothing on its own: an alarm with no hold
        // is decided from a still-reporting series, so it says Pending here and
        // a sample decides it.
        record("queue.depth.update", 2.0, -chrono::Duration::hours(1));
        let id = create(&url, "alarm-cli-update", "queue.depth.update", "gt", 10.0).await;

        let r = cli(
            &url,
            &["alarm", "update", "alarm-cli-update", "--threshold", "1"],
        )
        .await;
        assert_eq!(r.code, 0, "{}", r.err);
        assert!(
            r.out.contains("alarm \"alarm-cli-update\" updated"),
            "{}",
            r.out
        );
        let (_, v) = call(&url, "GET", "/gauge/alarms/alarm-cli-update", None).await;
        assert_eq!(v["spec"]["threshold"], 1.0);
        assert_eq!(v["spec"]["metric"], "queue.depth.update");
        assert_eq!(v["spec"]["comparison"], "gt");
        assert_eq!(v["spec"]["name"], "alarm-cli-update");
        // The point that crosses is an hour old: no hold is asked for yet, and
        // an alarm with no hold wants a still-reporting series, so the record
        // says Pending until a sample arrives.
        assert_eq!(v["status"]["phase"], "Pending");
        // A run of crossing points spanning the sixty seconds the alarm asks
        // for is what makes it fire. It fires at the creation, from the
        // stamps, and a hold longer than the window can reach is refused with
        // the server's own message naming the longest hold possible.
        let deep = "queue.backlog.deep";
        for age in (0..8).rev() {
            record(deep, 0.5, -chrono::Duration::minutes(15 * age));
        }
        let r = cli(
            &url,
            &[
                "alarm",
                "create",
                "alarm-cli-held",
                "--metric",
                deep,
                "--comparison",
                "lt",
                "--threshold",
                "1",
                "--for-seconds",
                "60",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{}", r.err);
        let (_, v) = call(&url, "GET", "/gauge/alarms/alarm-cli-held", None).await;
        assert_eq!(v["status"]["phase"], "Firing", "{v}");

        let too_long = gauge::MAX_HOLD.num_seconds() + 1;
        let r = cli(
            &url,
            &[
                "alarm",
                "update",
                "alarm-cli-held",
                "--for-seconds",
                &too_long.to_string(),
            ],
        )
        .await;
        assert_eq!(r.code, 1, "{r:?}");
        assert!(
            r.err.contains("for_seconds")
                && r.err.contains(&gauge::MAX_HOLD.num_seconds().to_string()),
            "{r:?}"
        );
        let (_, v) = call(&url, "GET", "/gauge/alarms/alarm-cli-held", None).await;
        assert_eq!(v["spec"]["for_seconds"], 60, "{v}");

        let r = cli(
            &url,
            &[
                "alarm",
                "update",
                &id,
                "--metric",
                "queue.backlog.update",
                "--comparison",
                "lt",
                "--for-seconds",
                "60",
                "--description",
                "watching the backlog",
                "--label",
                "team=core",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{}", r.err);
        let (_, v) = call(&url, "GET", &format!("/gauge/alarms/{id}"), None).await;
        assert_eq!(v["spec"]["metric"], "queue.backlog.update");
        assert_eq!(v["spec"]["comparison"], "lt");
        assert_eq!(v["spec"]["for_seconds"], 60);
        assert_eq!(v["spec"]["threshold"], 1.0);
        assert_eq!(v["description"], "watching the backlog");
        assert_eq!(v["labels"]["team"], "core");

        // Nothing named is a usage error.
        let before = cli(&url, &["alarm", "show", "alarm-cli-update", "-o", "json"]).await;
        let r = cli(&url, &["alarm", "update", "alarm-cli-update"]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert_eq!(
            r.err.trim(),
            "error: nothing to change: name a field to update"
        );
        let after = cli(&url, &["alarm", "show", "alarm-cli-update", "-o", "json"]).await;
        assert_eq!(before.out, after.out, "the record is untouched");

        // A threshold that is not a finite number is refused before anything is
        // sent, so the record cannot be half-changed by it.
        for bad in ["NaN", "inf", "ten"] {
            let r = cli(
                &url,
                &["alarm", "update", "alarm-cli-update", "--threshold", bad],
            )
            .await;
            assert_eq!(r.code, 2, "{r:?}");
            assert!(
                r.err.contains("finite number") || r.err.contains("not a number"),
                "{r:?}"
            );
        }
        let after = cli(&url, &["alarm", "show", "alarm-cli-update", "-o", "json"]).await;
        assert_eq!(before.out, after.out, "the record is untouched");

        let r = cli(&url, &["alarm", "update", "nope", "--threshold", "1"]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert_eq!(r.err.trim(), "error: alarm 'nope' not found");
    })
    .await
}

#[tokio::test]
async fn alarm_delete_resolves_the_name_and_needs_yes_without_a_terminal() {
    at_the_window(async {
        let url = serve().await;
        post(&url, "queue.depth.delete", 2.0).await;
        create(&url, "alarm-cli-delete", "queue.depth.delete", "gt", 10.0).await;

        // No terminal: it refuses rather than deleting.
        let r = cli(&url, &["alarm", "delete", "alarm-cli-delete"]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert!(r.err.contains("refusing to delete"), "{}", r.err);
        let (st, _) = call(&url, "GET", "/gauge/alarms/alarm-cli-delete", None).await;
        assert_eq!(st, StatusCode::OK, "still there");

        // A terminal that answers no.
        let r = pi(&url, "n\n", true, &["alarm", "delete", "alarm-cli-delete"]).await;
        assert_eq!(r.code, 0, "{r:?}");
        assert!(r.err.contains("aborted"), "{}", r.err);
        let (st, _) = call(&url, "GET", "/gauge/alarms/alarm-cli-delete", None).await;
        assert_eq!(st, StatusCode::OK, "still there");

        // A terminal that answers yes.
        let r = pi(&url, "y\n", true, &["alarm", "delete", "alarm-cli-delete"]).await;
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(r.out.trim(), "alarm \"alarm-cli-delete\" deleted");
        let (st, _) = call(&url, "GET", "/gauge/alarms/alarm-cli-delete", None).await;
        assert_eq!(st, StatusCode::NOT_FOUND, "gone");

        // --yes carries it, and the alias works.
        create(&url, "alarm-cli-delete2", "queue.depth.delete", "gt", 10.0).await;
        let r = cli(&url, &["alarm", "rm", "alarm-cli-delete2", "--yes"]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        assert_eq!(r.out.trim(), "alarm \"alarm-cli-delete2\" deleted");

        let r = cli(&url, &["alarm", "delete", "nope", "--yes"]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert_eq!(r.err.trim(), "error: alarm 'nope' not found");
    })
    .await
}

#[tokio::test]
async fn metric_list_shows_one_row_per_series() {
    at_the_window(async {
        let url = serve().await;
        // The window is shared by the process, so everything here is asked
        // for by name and only its own series are looked at.
        for (i, v) in [4.0, 9.0, 12.0, 7.0].iter().enumerate() {
            record(
                &mine("listed", None),
                *v,
                chrono::Duration::milliseconds(i as i64),
            );
        }
        record(
            &hosted("listed", "h1"),
            1.25,
            chrono::Duration::milliseconds(4),
        );
        record(
            &hosted("listed", "h2"),
            3.5,
            chrono::Duration::milliseconds(5),
        );
        let named = format!("--name=queue.{}", "listed");

        let r = cli(&url, &["metric", "list", &named]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        let table = rows(&r.out);
        assert_eq!(
            table[0],
            ["NAME", "SERIES", "LAST-VALUE", "AGE"],
            "{}",
            r.out
        );
        // One row per series, each showing its newest point, in the order the
        // window keeps its series. Other tests' points are in the window too:
        // the row is only checked where this test's own name says it should be.
        assert_eq!(table.len(), 4, "{}", r.out);
        assert_eq!(table[1][1], mine("listed", None));
        assert_eq!(table[1][2], "7.0");
        assert_eq!(table[2][1], hosted("listed", "h1"));
        assert_eq!(table[2][2], "1.25");
        assert_eq!(table[3][1], hosted("listed", "h2"));
        assert_eq!(table[3][2], "3.5");

        let r = cli(&url, &["metric", "ls", &named, "-o", "json"]).await;
        let items: Vec<Value> = serde_json::from_str(&r.out).unwrap();
        assert_eq!(items.len(), 3);
        assert_eq!(items[0]["last_value"], 7.0);
        assert_eq!(items[1]["labels"], json!("host=h1"));
        assert_eq!(items[2]["last_value"], 3.5);

        let r = cli(&url, &["metric", "ls", &named, "-o", "yaml"]).await;
        let back: Vec<Value> = serde_yaml_ng::from_str(&r.out).unwrap();
        assert_eq!(back.len(), 3);

        // A series is named by its whole key, which is what one shows it with.
        let r = cli(&url, &["metric", "ls", &named, "-o", "name"]).await;
        assert_eq!(
            r.out,
            format!(
                "{}\n{}\n{}\n",
                mine("listed", None),
                hosted("listed", "h1"),
                hosted("listed", "h2")
            )
        );

        let r = cli(&url, &["metric", "list", &named, "-o", "wide"]).await;
        assert!(r.out.contains("LABELS"), "{}", r.out);
        assert!(r.out.contains("host=h1"), "{}", r.out);

        // The name is matched whole: a series that merely starts with it stays out.
        record(
            "queue.listed-elsewhere",
            1.0,
            chrono::Duration::milliseconds(6),
        );
        let r = cli(&url, &["metric", "list", &named]).await;
        assert!(!r.out.contains("queue.listed-elsewhere"), "{}", r.out);

        // A name with nothing held for it is a table with no rows, not an error.
        let r = cli(&url, &["metric", "list", "--name", "nothing.at.all"]).await;
        assert_eq!(r.code, 0, "{r:?}");
        assert_eq!(rows(&r.out).len(), 1, "{}", r.out);
        let r = cli(
            &url,
            &["metric", "list", "--name", "nothing.at.all", "-o", "json"],
        )
        .await;
        assert_eq!(r.out.trim(), "[]");
    })
    .await
}

#[tokio::test]
async fn metric_show_prints_the_points_of_one_series() {
    at_the_window(async {
        let url = serve().await;
        let plain = mine("shown", None);
        let labelled = hosted("shown", "h2");
        for (i, v) in [4.0, 9.0, 12.0, 7.0].iter().enumerate() {
            record(&plain, *v, chrono::Duration::milliseconds(i as i64));
        }
        record(&labelled, 3.5, chrono::Duration::milliseconds(4));

        let r = cli(&url, &["metric", "show", &plain]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        let table = rows(&r.out);
        assert_eq!(table[0], ["TIME", "VALUE"], "{}", r.out);
        assert_eq!(table.len(), 5, "{}", r.out);
        assert_eq!(table[1].last().unwrap(), "4.0");
        assert_eq!(table[4].last().unwrap(), "7.0");

        let r = cli(&url, &["metric", "show", &plain, "--last", "2"]).await;
        let table = rows(&r.out);
        assert_eq!(table[0], ["TIME", "VALUE"], "{}", r.out);
        assert_eq!(table.len(), 3, "{}", r.out);
        assert_eq!(table[1].last().unwrap(), "12.0");
        assert_eq!(table[2].last().unwrap(), "7.0");

        // More asked for than there are is not an error.
        let r = cli(&url, &["metric", "show", &plain, "--last", "99"]).await;
        assert_eq!(rows(&r.out).len(), 5, "{}", r.out);

        // A key with labels reaches the server whole, and selects only that series.
        let r = cli(&url, &["metric", "show", &labelled]).await;
        assert_eq!(r.code, 0, "{}", r.err);
        let table = rows(&r.out);
        assert_eq!(table.len(), 2, "{}", r.out);
        assert_eq!(table[1].last().unwrap(), "3.5");

        let r = cli(&url, &["metric", "show", &plain, "-o", "json"]).await;
        let points: Vec<Value> = serde_json::from_str(&r.out).unwrap();
        assert_eq!(points.len(), 4);
        assert_eq!(points[0]["metric"], plain);
        assert_eq!(points[3]["value"], 7.0);

        let r = cli(&url, &["metric", "show", &plain, "-o", "yaml"]).await;
        let back: Vec<Value> = serde_yaml_ng::from_str(&r.out).unwrap();
        assert_eq!(back.len(), 4);

        let r = cli(&url, &["metric", "get", &plain, "-o", "name"]).await;
        assert_eq!(r.out.matches('\n').count(), 4, "{}", r.out);

        let r = cli(&url, &["metric", "show", "nothing.here.at.all"]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert_eq!(
            r.err.trim(),
            "error: the server holds no points for series 'nothing.here.at.all'"
        );
    })
    .await
}

#[tokio::test]
async fn the_two_nouns_are_wired_in_and_not_shadowed() {
    let url = serve().await;
    let r = cli(&url, &["alarm", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = cli(&url, &["metric", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = cli(&url, &["alarm", "frobnicate"]).await;
    assert_eq!(r.code, 2, "{r:?}");
    assert!(r.err.contains("unrecognized subcommand"), "{}", r.err);

    // Completion knows both nouns: it renders the whole tree, so this fails
    // if either is wired in a way clap dislikes.
    let r = cli(&url, &["completion", "bash"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.contains("alarm"), "completion misses the noun");
    assert!(r.out.contains("metric"), "completion misses the noun");
}
