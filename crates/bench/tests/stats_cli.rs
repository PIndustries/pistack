// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi machine stats` and `pi host stats` against a small in-process
//! server that answers with a fixed report.

use axum::{extract::Path, routing::get, Json, Router};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::io::Cursor;

async fn machine_stats(Path(id): Path<String>) -> Json<Value> {
    if id == "idle" {
        return Json(json!({"id": "mach-2", "name": "idle", "series": []}));
    }
    let labels = json!({"machine": "mach-1", "name": "web", "project": "proj-1", "host": "h1"});
    let on = |extra: (&str, &str)| {
        let mut all = labels.clone();
        all[extra.0] = json!(extra.1);
        all
    };
    let at = "2026-11-12T08:00:00Z";
    Json(json!({
        "id": "mach-1",
        "name": "web",
        "host": "h1",
        "series": [
            {"metric": "pistack_machine_cpu_seconds_total", "labels": labels,
             "value": 5400.0, "timestamp": at, "rate": 0.5},
            {"metric": "pistack_machine_cpu_usage_cores", "labels": labels,
             "value": 0.5, "timestamp": at},
            {"metric": "pistack_machine_memory_bytes", "labels": labels,
             "value": 1073741824.0, "timestamp": at},
            {"metric": "pistack_machine_disk_written_bytes_total", "labels": on(("device", "vda")),
             "value": 3221225472.0, "timestamp": at, "rate": 2048.0},
            {"metric": "pistack_machine_disk_writes_total", "labels": on(("device", "vda")),
             "value": 1200.0, "timestamp": at, "rate": 3.0},
            {"metric": "pistack_machine_network_receive_bytes_total",
             "labels": on(("interface", "vnet2")), "value": 500.0, "timestamp": at}
        ]
    }))
}

async fn host_stats() -> Json<Value> {
    let labels = json!({"host": "rack1"});
    let at = "2026-11-12T08:00:00Z";
    Json(json!({
        "id": "host-1",
        "name": "rack1",
        "host": "rack1",
        "series": [
            {"metric": "pistack_host_load1", "labels": labels, "value": 1.25, "timestamp": at},
            {"metric": "pistack_host_memory_mib", "labels": labels, "value": 16384.0, "timestamp": at},
            {"metric": "pistack_host_data_disk_free_bytes", "labels": labels,
             "value": 107374182400.0, "timestamp": at},
            {"metric": "pistack_host_machines", "labels": labels, "value": 3.0, "timestamp": at}
        ]
    }))
}

async fn serve() -> String {
    let app = Router::new()
        .route("/compute/machines/{id}/stats", get(machine_stats))
        .route("/compute/hosts/{id}/stats", get(host_stats));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    format!("http://{addr}")
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

#[tokio::test]
async fn machine_stats_prints_values_in_their_units_and_rates_for_counters() {
    let url = serve().await;
    let r = pi(&url, &["machine", "stats", "web"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let table = rows(&r.out);
    assert_eq!(table[0], ["METRIC", "VALUE", "RATE"], "{}", r.out);
    assert_eq!(table[1], ["cpu_seconds_total", "5400.0 s", "0.50 cpus"]);
    assert_eq!(table[2], ["cpu_usage_cores", "0.50"]);
    assert_eq!(table[3], ["memory_bytes", "1.0 GiB"]);
    assert_eq!(
        table[4],
        ["disk_written_bytes_total (vda)", "3.0 GiB", "2.0 KiB/s"]
    );
    assert_eq!(table[5], ["disk_writes_total (vda)", "1200", "3/s"]);
    assert_eq!(table[6], ["network_receive_bytes_total (vnet2)", "500 B"]);

    // JSON is the report as the server sent it.
    let r = pi(&url, &["machine", "stats", "web", "-o", "json"]).await;
    let report: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(report["host"], "h1");
    assert_eq!(report["series"].as_array().unwrap().len(), 6);
}

#[tokio::test]
async fn a_machine_with_no_usage_says_so_and_is_not_an_error() {
    let url = serve().await;
    let r = pi(&url, &["machine", "stats", "idle"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.out.is_empty(), "{}", r.out);
    assert!(r.err.contains("no usage reported"), "{}", r.err);
}

#[tokio::test]
async fn host_stats_prints_load_memory_and_disk() {
    let url = serve().await;
    let r = pi(&url, &["host", "stats", "rack1"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let table = rows(&r.out);
    assert_eq!(table[1], ["load1", "1.25"]);
    assert_eq!(table[2], ["memory_mib", "16.0 GiB"]);
    assert_eq!(table[3], ["data_disk_free_bytes", "100.0 GiB"]);
    assert_eq!(table[4], ["machines", "3"]);
}
