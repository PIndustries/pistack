// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi bucket` and `pi object` against the object service itself, served
//! in-process at `/silo` the way the gateway mounts it. A middleware
//! attaches the caller, standing in for the token check the gateway does.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    response::{IntoResponse, Response},
    Router,
};
use bench::cli::{self, Io};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id};
use serde_json::Value;
use silo::Client;
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const BAD_TOKEN: &str = "tok-wrong";

#[derive(Clone)]
struct World {
    caller: Caller,
    /// Requests that came in without the good token.
    refused: Arc<Mutex<Vec<String>>>,
}

async fn admit(State(w): State<World>, mut req: Request, next: Next) -> Response {
    let bearer = req
        .headers()
        .get("authorization")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .unwrap_or("");
    if bearer != "t" {
        w.refused
            .lock()
            .unwrap()
            .push(format!("{} {}", req.method(), req.uri()));
        return (
            StatusCode::UNAUTHORIZED,
            axum::Json(serde_json::json!({
                "error": {"code": "unauthorized", "message": "no credentials", "details": {}}
            })),
        )
            .into_response();
    }
    req.extensions_mut().insert(w.caller.clone());
    next.run(req).await
}

/// Serve `pi` the object service at `/silo`, with a caller attached for
/// every request that carries the test token.
async fn serve() -> (String, World) {
    let dir = tempfile::tempdir().expect("data dir");
    let client = Client::new(Store::in_memory().unwrap(), dir.path().join("silo"));
    let world = World {
        caller: Caller {
            principal: Id::new(kind::PRINCIPAL),
            principal_name: "admin".into(),
            project: None,
            roles: vec!["admin".into()],
            system: true,
        },
        refused: Default::default(),
    };
    let app = Router::new()
        .nest(
            "/silo",
            silo::http::router(client).layer(middleware::from_fn_with_state(world.clone(), admit)),
        )
        // Anything else is a bug in the CLI's paths, said loudly.
        .fallback(|req: Request| async move {
            (
                StatusCode::NOT_FOUND,
                format!("unhandled {} {}", req.method(), req.uri()),
            )
        });
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), world)
}

struct Run {
    code: i32,
    out: Vec<u8>,
    err: String,
}

impl Run {
    fn text(&self) -> String {
        String::from_utf8_lossy(&self.out).into_owned()
    }
    /// Table output past the header row.
    fn rows(&self) -> Vec<String> {
        self.text().lines().skip(1).map(str::to_owned).collect()
    }
}

impl std::fmt::Debug for Run {
    /// The bytes of a streamed get are not printed; the tail of a broken
    /// pipe would bury the assertion.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Run")
            .field("code", &self.code)
            .field("out_len", &self.out.len())
            .field(
                "out_head",
                &self.text().chars().take(160).collect::<String>(),
            )
            .field("err", &self.err)
            .finish()
    }
}

/// `tokio` blocks the runtime on `spawn_blocking` for file IO; a real pi
/// is multithreaded, so run the CLI the same way.
async fn pi(url: &str, args: &[&str]) -> Run {
    let url = url.to_owned();
    let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    tokio::task::spawn_blocking(move || {
        let dir = tempfile::tempdir().unwrap();
        let mut argv: Vec<String> = [
            "pi",
            "--config",
            dir.path().join("config.toml").to_str().unwrap(),
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();
        argv.extend(["--url".into(), url, "--token".into(), "t".into()]);
        argv.extend(args);
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
            futures::executor::block_on(cli::run(argv, &mut io)).expect("run")
        };
        Run {
            code,
            out,
            err: String::from_utf8(err).unwrap(),
        }
    })
    .await
    .expect("spawn")
}

fn json_of(out: &str) -> Value {
    serde_json::from_str(out).expect("json output")
}

#[tokio::test]
async fn bucket_crud_over_the_cli() {
    let (url, _w) = serve().await;

    let r = pi(&url, &["bucket", "create", "photos"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.text().starts_with("bucket \"photos\" created"),
        "{}",
        r.text()
    );

    let r = pi(&url, &["bucket", "create", "archive", "--versioning"]).await;
    assert_eq!(r.code, 0, "{}", r.err);

    let r = pi(&url, &["bucket", "list"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let mut rows = r.rows();
    assert_eq!(rows.len(), 2, "{r:?}");
    rows.sort();
    assert!(
        rows[0].contains("archive") && rows[0].contains("enabled"),
        "{r:?}"
    );
    assert!(
        rows[1].contains("photos") && rows[1].contains("suspended"),
        "{r:?}"
    );

    // Name or id, and every -o shape the other nouns answer with.
    let shown = pi(&url, &["bucket", "show", "photos"]).await;
    assert_eq!(shown.code, 0, "{}", shown.err);
    let as_json = pi(&url, &["bucket", "show", "photos", "-o", "json"]).await;
    assert_eq!(as_json.code, 0, "{}", as_json.err);
    let v = json_of(&as_json.text());
    assert_eq!(v["name"], "photos");
    let id = v["id"].as_str().unwrap().to_owned();
    assert!(id.starts_with("bkt-"), "{id}");

    let by_id = pi(&url, &["bucket", "show", &id]).await;
    assert_eq!(by_id.code, 0, "{}", by_id.err);
    assert!(by_id.text().contains("photos"), "{}", by_id.text());

    let as_yaml = pi(&url, &["bucket", "show", "photos", "-o", "yaml"]).await;
    assert_eq!(as_yaml.code, 0, "{}", as_yaml.err);
    assert!(
        as_yaml.text().contains("name: photos"),
        "{}",
        as_yaml.text()
    );

    let wide = pi(&url, &["bucket", "list", "-o", "wide"]).await;
    assert!(wide.text().contains("FULL-ID"), "{}", wide.text());

    // Delete answers with no body; --yes is required without a terminal.
    let r = pi(&url, &["bucket", "delete", "archive"]).await;
    assert_ne!(r.code, 0, "{r:?}");
    assert!(r.err.contains("--yes"), "{}", r.err);

    let r = pi(&url, &["bucket", "delete", "archive", "--yes"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.text(), "bucket \"archive\" deleted\n", "{}", r.text());

    let r = pi(&url, &["bucket", "show", "archive"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    let r = pi(&url, &["bucket", "show", "nope"]).await;
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn object_round_trip_streams_both_ways() {
    let (url, _w) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    // Past the client's 256 KiB read chunk, so a put is several chunks up
    // and a get is several chunks back.
    let src = dir.path().join("src.bin");
    let bytes: Vec<u8> = (0..600 * 1024).map(|i| (i % 251) as u8).collect();
    std::fs::write(&src, &bytes).unwrap();
    let expected = {
        use sha2::{Digest, Sha256};
        Sha256::digest(&bytes)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>()
    };

    let r = pi(&url, &["bucket", "create", "data"]).await;
    assert_eq!(r.code, 0, "{}", r.err);

    let r = pi(
        &url,
        &[
            "object",
            "put",
            "data",
            "dir/report.bin",
            &src.display().to_string(),
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(
        r.text().contains(&format!(
            "object \"dir/report.bin\" uploaded ({} bytes)",
            bytes.len()
        )),
        "{}",
        r.text()
    );

    let r = pi(&url, &["object", "list", "data"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let line = &r.rows()[0];
    assert!(line.contains("dir/report.bin"), "{line}");
    assert!(line.contains(&bytes.len().to_string()), "{line}");
    assert!(line.contains(&expected), "{line}");

    // Without `--out` the bytes land in stdout untouched, byte for byte.
    let r = pi(&url, &["object", "get", "data", "dir/report.bin"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(r.out, bytes);

    // `--out FILE` writes the file and reports it.
    let out = dir.path().join("copy.bin");
    let r = pi(
        &url,
        &[
            "object",
            "get",
            "data",
            "dir/report.bin",
            "--out",
            out.to_str().unwrap(),
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert!(r.text().contains("written to"), "{}", r.text());
    assert_eq!(std::fs::read(&out).unwrap(), bytes);

    // A second put replaces the bytes and the version moves.
    let small = dir.path().join("small.txt");
    std::fs::write(&small, b"second try").unwrap();
    let r = pi(
        &url,
        &[
            "object",
            "put",
            "data",
            "dir/report.bin",
            &small.display().to_string(),
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    let r = pi(&url, &["object", "list", "data", "-o", "json"]).await;
    let items = json_of(&r.text());
    assert_eq!(items[0]["key"], "dir/report.bin");
    assert_eq!(items[0]["version"], 2);
    assert_eq!(items[0]["size_bytes"], 10);

    let r = pi(
        &url,
        &["object", "delete", "data", "dir/report.bin", "--yes"],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);
    assert_eq!(
        r.text(),
        "object \"dir/report.bin\" deleted\n",
        "{}",
        r.text()
    );

    let r = pi(&url, &["object", "list", "data"]).await;
    assert_eq!(r.rows().len(), 0, "{r:?}");

    let r = pi(&url, &["object", "get", "data", "dir/report.bin"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("error:"), "{}", r.err);
}

#[tokio::test]
async fn object_prefix_lookup_and_bad_keys() {
    let (url, _w) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("f.txt");
    std::fs::write(&f, b"hi").unwrap();
    let f = f.display().to_string();

    let r = pi(&url, &["bucket", "create", "deep", "--versioning"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    for key in ["a/one.txt", "a/two.txt", "b/one.txt"] {
        let r = pi(&url, &["object", "put", "deep", key, &f]).await;
        assert_eq!(r.code, 0, "{key}: {}", r.err);
    }

    let r = pi(&url, &["object", "list", "deep"]).await;
    assert_eq!(r.rows().len(), 3, "{r:?}");

    let r = pi(&url, &["object", "list", "deep", "--prefix", "a/"]).await;
    assert_eq!(r.rows().len(), 2, "{r:?}");
    assert!(r.rows().iter().all(|l| l.contains("a/")), "{r:?}");

    let r = pi(&url, &["object", "list", "deep", "--prefix", "zz"]).await;
    assert_eq!(r.rows().len(), 0, "{r:?}");

    // `-o name` gives one key per line, no table.
    let r = pi(
        &url,
        &["object", "list", "deep", "-o", "name", "--prefix", "b/"],
    )
    .await;
    assert_eq!(r.text(), "b/one.txt\n", "{r:?}");

    // A bad key is refused before anything goes on the wire.
    for key in ["/etc/passwd", "../escape", "a//b"] {
        let r = pi(&url, &["object", "put", "deep", key, &f]).await;
        assert_eq!(r.code, 1, "{key}: ({} {})", r.code, r.err);
        assert!(r.err.contains("relative path"), "{key}: {}", r.err);
    }
    let r = pi(&url, &["object", "get", "deep", "../escape"]).await;
    assert_eq!(r.code, 1, "{r:?}");

    // A missing key is the service's own 404, passed through as an error.
    let r = pi(&url, &["object", "get", "deep", "nope.txt"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("error:"), "{}", r.err);

    // An unknown bucket is an error, not an empty list.
    let r = pi(&url, &["object", "list", "nosuch"]).await;
    assert_eq!(r.code, 1, "{r:?}");
}

#[tokio::test]
async fn a_missing_token_is_passed_through() {
    let (url, w) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let args: Vec<String> = [
        "pi",
        "--config",
        dir.path().join("config.toml").to_str().unwrap(),
        "--url",
        &url,
        "--token",
        BAD_TOKEN,
        "bucket",
        "list",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let r = tokio::task::spawn_blocking(move || {
        let mut input = Cursor::new(Vec::new());
        let mut out = Vec::new();
        let mut err = Vec::new();
        let mut io = Io {
            stdin: &mut input,
            stdout: &mut out,
            stderr: &mut err,
            tty: false,
        };
        let code = futures::executor::block_on(cli::run(args, &mut io)).expect("run");
        Run {
            code,
            out,
            err: String::from_utf8(err).unwrap(),
        }
    })
    .await
    .unwrap();
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("no credentials"), "{}", r.err);
    assert_eq!(w.refused.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn object_delete_without_yes_aborts() {
    let (url, _w) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("f.txt");
    std::fs::write(&f, b"hi").unwrap();

    let r = pi(&url, &["bucket", "create", "keep"]).await;
    assert_eq!(r.code, 0, "{}", r.err);
    let r = pi(
        &url,
        &["object", "put", "keep", "a.txt", &f.display().to_string()],
    )
    .await;
    assert_eq!(r.code, 0, "{}", r.err);

    // No terminal to confirm on, so the delete is refused, not run.
    let r = pi(&url, &["object", "delete", "keep", "a.txt"]).await;
    assert_ne!(r.code, 0, "{r:?}");
    assert!(r.err.contains("--yes"), "{}", r.err);

    let r = pi(&url, &["object", "list", "keep"]).await;
    assert_eq!(r.rows().len(), 1, "{r:?}");
}
