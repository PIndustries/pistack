// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi keypair` against a small in-process server, and the parts of
//! `pi machine create` that name a pair. The private key a create returns
//! only ever goes to a file, so the file is what these tests read.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::IntoResponse,
    routing::{delete, get, post},
    Json, Router,
};
use bench::cli::{self, Io};
use serde_json::{json, Value};
use std::io::Cursor;
use std::sync::{Arc, Mutex};

const PRIVATE: &str =
    "-----BEGIN OPENSSH PRIVATE KEY-----\nb3BlbnNzaQ\n-----END OPENSSH PRIVATE KEY-----\n";
const PUBLIC: &str =
    "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG6NiWNsbUCDY1DqQxFpj7Y1p+Ph+ovvRShuoREjxj/7 demo@box";

#[derive(Clone)]
struct Mock {
    /// Bodies of the `POST /compute/keypairs` calls, in order.
    creates: Arc<Mutex<Vec<Value>>>,
    /// Bodies of the `POST /compute/machines` calls, in order.
    machines: Arc<Mutex<Vec<Value>>>,
    /// What `GET /compute/machines/{id}` answers.
    machine: Arc<Mutex<Option<Value>>>,
    /// Paths of the `DELETE /compute/keypairs/{id}` calls, in order.
    deletes: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    fn last_create(&self) -> Value {
        self.creates
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a create")
    }
    fn last_machine(&self) -> Value {
        self.machines
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a create")
    }
}

fn stored(name: &str) -> Value {
    json!({
        "id": "keypair-0199a5e0-1111-7000-8000-000000000001",
        "name": name,
        "project": "proj-0199a5e0-2222-7000-8000-000000000002",
        "spec": {
            "public_key": PUBLIC,
            "key_type": "ssh-ed25519",
            "fingerprint": "SHA256:kZaHsQEOwFK+9RD5E9L3VAB8FaS9MZYfNAMVbxg0f1I",
        },
        "status": {},
        "created_at": "2026-01-01T00:00:00Z",
    })
}

async fn create_keypair(
    State(m): State<Mock>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    let imported = body.get("public_key").is_some();
    m.creates.lock().unwrap().push(body.clone());
    // Like the server: a request with a key imports it, one without makes a
    // pair and returns its private half once.
    let mut v = stored("made");
    if imported {
        v["name"] = body["name"].clone();
    } else {
        v["private_key"] = json!(PRIVATE);
    }
    (StatusCode::CREATED, Json(v)).into_response()
}

async fn delete_keypair(State(m): State<Mock>, Path(id): Path<String>) -> StatusCode {
    m.deletes
        .lock()
        .unwrap()
        .push(format!("/compute/keypairs/{id}"));
    StatusCode::NO_CONTENT
}

async fn list_keypairs() -> Json<Value> {
    Json(json!({"items": [stored("laptop")], "total": 1}))
}

async fn create_machine(
    State(m): State<Mock>,
    Json(body): Json<Value>,
) -> axum::response::Response {
    m.machines.lock().unwrap().push(body.clone());
    let mut v = body;
    v["id"] = json!("mach-0199a5e0-1111-7000-8000-000000000009");
    v["status"] = json!({"phase": "pending"});
    *m.machine.lock().unwrap() = Some(v.clone());
    (StatusCode::CREATED, Json(v)).into_response()
}

async fn show_machine(State(m): State<Mock>, Path(id): Path<String>) -> axum::response::Response {
    match m.machine.lock().unwrap().as_ref() {
        Some(v) if v["id"] == id.as_str() => Json(v.clone()).into_response(),
        _ => (
            StatusCode::NOT_FOUND,
            Json(json!({"error": {"code": "not_found", "message": "no such machine", "details": {}}})),
        )
            .into_response(),
    }
}

async fn serve() -> (String, Mock) {
    let mock = Mock {
        creates: Arc::new(Mutex::new(Vec::new())),
        machines: Arc::new(Mutex::new(Vec::new())),
        machine: Arc::new(Mutex::new(None)),
        deletes: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route("/compute/keypairs", get(list_keypairs).post(create_keypair))
        .route("/compute/keypairs/{id}", delete(delete_keypair))
        .route("/compute/machines", post(create_machine))
        .route("/compute/machines/{id}", get(show_machine))
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

async fn pi(url: &str, args: &[&str]) -> Run {
    run(url, args, false).await
}

async fn pi_raced(url: &str, args: &[&str]) -> Run {
    run(url, args, false).await
}

async fn run(url: &str, args: &[&str], tty: bool) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let mut argv: Vec<String> = [
        "pi",
        "--config",
        dir.path().join("config.toml").to_str().unwrap(),
        "--url",
        url,
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

#[tokio::test]
async fn create_writes_a_generated_key_to_the_named_file() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("id_ed25519");

    let r = pi(
        &url,
        &["keypair", "create", "made", "--out", key.to_str().unwrap()],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("keypair \"made\" created"), "{}", r.out);
    assert!(r.out.contains("private key written to"), "{}", r.out);
    // The key never travels through stdout, where it would land in a log.
    assert!(!r.out.contains("BEGIN OPENSSH PRIVATE KEY"), "{}", r.out);
    assert_eq!(std::fs::read_to_string(&key).unwrap(), PRIVATE);

    // The request carried no key, which is what asks the server to make one.
    assert_eq!(mock.last_create(), json!({"name": "made"}));

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&key).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "the key is readable by others");
    }
}

#[tokio::test]
async fn create_refuses_to_replace_a_file_that_exists() {
    let (url, mock) = serve().await;
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("id_ed25519");
    std::fs::write(&key, "someone else's key").unwrap();

    let r = pi(
        &url,
        &["keypair", "create", "made", "--out", key.to_str().unwrap()],
    )
    .await;
    assert_ne!(r.code, 0, "{r:?}");
    assert!(r.err.contains("refusing to overwrite"), "{}", r.err);
    assert_eq!(std::fs::read_to_string(&key).unwrap(), "someone else's key");
    // Nothing was asked of the server, so no pair exists without a key.
    assert!(mock.creates.lock().unwrap().is_empty());

    // A directory that is not there is found out the same way.
    let r = pi(
        &url,
        &[
            "keypair",
            "create",
            "made",
            "--out",
            dir.path().join("nope/id_ed25519").to_str().unwrap(),
        ],
    )
    .await;
    assert_ne!(r.code, 0, "{r:?}");
    assert!(r.err.contains("no such directory"), "{}", r.err);
    assert!(mock.creates.lock().unwrap().is_empty());
}

/// A create whose reply leaves `target` taken between the CLI's check and its
/// write. That race is the only way a pair can be made without its key
/// reaching the disk, and it is what the rollback covers.
async fn serve_raced(race: &std::path::Path) -> (String, Mock) {
    let filler = race.to_path_buf();
    let mock = Mock {
        creates: Arc::new(Mutex::new(Vec::new())),
        machines: Arc::new(Mutex::new(Vec::new())),
        machine: Arc::new(Mutex::new(None)),
        deletes: Arc::new(Mutex::new(Vec::new())),
    };
    let app = Router::new()
        .route(
            "/compute/keypairs",
            post(move |State(m): State<Mock>, Json(body): Json<Value>| {
                let filler = filler.clone();
                async move {
                    let name = body["name"].as_str().unwrap_or("").to_owned();
                    m.creates.lock().unwrap().push(body);
                    // The name the key is meant for, filled in as if by
                    // another process in the instant between the CLI's
                    // check and its write.
                    std::fs::write(&filler, "a key from elsewhere").unwrap();
                    let mut v = stored(&name);
                    v["private_key"] = json!(PRIVATE);
                    (StatusCode::CREATED, Json(v)).into_response()
                }
            }),
        )
        .route("/compute/keypairs/{id}", delete(delete_keypair))
        .with_state(mock.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (format!("http://{addr}"), mock)
}

#[tokio::test]
async fn a_pair_whose_key_cannot_be_written_is_deleted_again() {
    let dir = tempfile::tempdir().unwrap();
    let key = dir.path().join("id_ed25519");
    let (url, mock) = serve_raced(&key).await;

    let r = pi_raced(
        &url,
        &["keypair", "create", "made", "--out", key.to_str().unwrap()],
    )
    .await;

    assert_ne!(r.code, 0, "{r:?}");
    // The pair was made, the key could not be written, and the pair went back.
    assert!(r.err.contains("has been deleted again"), "{}", r.err);
    assert_eq!(mock.creates.lock().unwrap().len(), 1);
    assert_eq!(
        mock.deletes.lock().unwrap().as_slice(),
        ["/compute/keypairs/keypair-0199a5e0-1111-7000-8000-000000000001".to_string()]
    );
    // Whoever held the name keeps their file.
    assert_eq!(
        std::fs::read_to_string(&key).unwrap(),
        "a key from elsewhere"
    );
}

fn write_pub() -> String {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("id_ed25519.pub");
    std::fs::write(&path, format!("{PUBLIC}\n")).unwrap();
    // The directory is dropped with the test; leak it so the path outlives
    // the call that made it.
    std::mem::forget(dir);
    path.to_str().unwrap().to_owned()
}

async fn pi_tty(url: &str, args: &[&str]) -> Run {
    run(url, args, true).await
}

#[tokio::test]
async fn create_refuses_to_print_a_key_where_someone_can_see_it() {
    let (url, _mock) = serve().await;

    // With no terminal attached a key may go to stdout: a pipe, a script.
    let r = pi(&url, &["keypair", "create", "made"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("BEGIN OPENSSH PRIVATE KEY"), "{}", r.out);

    // On a terminal it may not, in any output format, and nothing is printed
    // to stdout either.
    for fmt in ["table", "json"] {
        let r = pi_tty(&url, &["keypair", "create", "made", "-o", fmt]).await;
        assert_ne!(r.code, 0, "{fmt}: {r:?}");
        assert!(r.err.contains("--out"), "{fmt}: {}", r.err);
        assert!(!r.out.contains("PRIVATE KEY"), "{fmt}: {}", r.out);
    }

    // An import on a terminal is reported as usual: there is no key to hide.
    let pub_file = write_pub();
    let r = pi_tty(
        &url,
        &["keypair", "create", "made", "--public-key-file", &pub_file],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("created"), "{}", r.out);
}

#[tokio::test]
async fn import_sends_the_key_from_the_file() {
    let (url, mock) = serve().await;
    let pub_file = write_pub();
    let r = pi(
        &url,
        &[
            "keypair",
            "create",
            "laptop",
            "--public-key-file",
            &pub_file,
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        mock.last_create(),
        json!({"name": "laptop", "public_key": PUBLIC})
    );
    // An import makes nothing, so there is no key to speak of.
    assert!(!r.out.contains("private key"), "{}", r.out);

    // Both flags at once is a contradiction, and nothing is sent.
    let dir = tempfile::tempdir().unwrap();
    let before = mock.creates.lock().unwrap().len();
    let r = pi(
        &url,
        &[
            "keypair",
            "create",
            "laptop",
            "--public-key-file",
            &pub_file,
            "--out",
            dir.path().join("k").to_str().unwrap(),
        ],
    )
    .await;
    assert_ne!(r.code, 0, "{r:?}");
    assert!(r.err.contains("--out"), "{}", r.err);
    assert_eq!(mock.creates.lock().unwrap().len(), before);

    // A file that is not there is reported as such.
    let r = pi(
        &url,
        &[
            "keypair",
            "create",
            "laptop",
            "--public-key-file",
            "/nope/id.pub",
        ],
    )
    .await;
    assert_ne!(r.code, 0);
    assert!(r.err.contains("reading /nope/id.pub"), "{}", r.err);
}

#[tokio::test]
async fn machine_show_prints_the_keypair() {
    let (url, _mock) = serve().await;
    let r = pi(
        &url,
        &[
            "machine",
            "create",
            "web",
            "--size",
            "s.1x1",
            "--image",
            "ubuntu",
            "--keypair",
            "laptop",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let id = "mach-0199a5e0-1111-7000-8000-000000000009";
    let r = pi(&url, &["machine", "show", id]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("spec.keypair"), "{}", r.out);
    assert!(r.out.contains("laptop"), "{}", r.out);
}

#[tokio::test]
async fn list_shows_the_type_and_fingerprint() {
    let (url, _mock) = serve().await;
    let r = pi(&url, &["keypair", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("laptop"), "{}", r.out);
    assert!(r.out.contains("ssh-ed25519"), "{}", r.out);
    assert!(r.out.contains("SHA256:kZaHsQEOwFK"), "{}", r.out);
}

#[tokio::test]
async fn machine_create_names_a_keypair() {
    let (url, mock) = serve().await;
    let base = [
        "machine", "create", "web", "--size", "s.1x1", "--image", "ubuntu",
    ];

    let r = pi(&url, &[&base[..], &["--keypair", "laptop"]].concat()).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(mock.last_machine()["spec"]["keypair"], "laptop");

    // Without the flag the field is not sent at all.
    let r = pi(&url, &base).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(mock.last_machine()["spec"].get("keypair").is_none());

    // It sits beside raw keys, which keep working.
    let r = pi(
        &url,
        &[
            &base[..],
            &["--keypair", "laptop", "--ssh-key", "ssh-ed25519 AAAAbox"],
        ]
        .concat(),
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let spec = &mock.last_machine()["spec"];
    assert_eq!(spec["keypair"], "laptop");
    assert_eq!(spec["ssh_keys"], json!(["ssh-ed25519 AAAAbox"]));
}
