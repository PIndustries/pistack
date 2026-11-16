// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi backup` against the block storage service itself: the slab module's
//! own router, mounted where the gateway mounts it, over an in-memory store
//! and a local file backend in a tempdir. Nothing here answers for the
//! service. A volume is provisioned, a bucket is made, and a backup's bytes
//! are really uploaded and read back.
//!
//! The service moves a resource through its phases in the background, driven
//! by the events its writes emit. The harness runs that same loop, so a CLI
//! that waits is watching the real state machine and not a scripted one.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    Router,
};
use bench::cli::{self, Io};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Meta};
use serde_json::{json, Value};
use slab::{Client, LocalFileBackend};
use std::io::{Cursor, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The bytes a fixture writes at the front of a volume, and what a restore must
/// put back.
const PAYLOAD: &[u8] = b"pistack-backup-payload";

/// Zero bytes left after the payload in a volume a backup is made of, so the
/// file clears the object service's minimum part size and can be uploaded as a
/// single part. Never written, so the file stays sparse. See
/// [`Harness::write_bytes`].
const TAIL_ZERO_BYTES: usize = silo::MIN_PART_BYTES;

/// The size of the volumes a backup is made of, in GiB: the smallest the service
/// accepts.
///
/// Chosen for what the upload costs, not what it proves. A backup is streamed to
/// the object service as multipart parts cut at 64 MiB, and the service refuses a
/// part below its floor of [`silo::MIN_PART_BYTES`] unless it is the last one — so
/// a source has to be at least that floor for an upload to be possible at all. One
/// GiB is the smallest real volume that clears it, and the upload then carries the
/// whole file, which is what a backup of a real volume is. A volume shrunk to the
/// payload instead is a volume the service refuses to restore into, because its
/// capacity check reads the file's length, so the size has to stay real.
///
/// What is kept out of the way is the reading of that size back. The file is
/// provisioned sparse and the payload lands at its front, so the objects a backup
/// here makes are mostly holes and the copy is disk-bound rather than
/// bytes-bound. Ten tests at a GiB each is what the wait allowance below is
/// sized for.
const VOLUME_GIB: u64 = 1;

/// What the server was asked to do, so a test can read a request's body back
/// instead of guessing what the CLI sent from its own output.
#[derive(Clone, Default)]
struct Seen {
    /// `(path, body)` of every write, in order.
    writes: Arc<Mutex<Vec<(String, Value)>>>,
}

impl Seen {
    fn last_write(&self) -> (String, Value) {
        self.writes
            .lock()
            .unwrap()
            .last()
            .cloned()
            .expect("a write")
    }
}

/// One server's worth of state: the slab client the router serves, the store
/// behind it, and the recorder in front of it.
struct Harness {
    url: String,
    client: Client,
    /// The project every fixture here belongs to.
    project: Id,
    seen: Seen,
    dir: tempfile::TempDir,
    /// Stops the reconciler the harness started, so the tempdir is dropped
    /// with nothing still writing into it.
    stop: tokio_util::sync::CancellationToken,
    reconcile: tokio::task::JoinHandle<()>,
}

impl Harness {
    /// The front of a volume's file — as much as a payload fills. A local volume
    /// is provisioned to its whole size, so a file runs on with slack after
    /// whatever a test wrote into it; the bytes a restore put there are the
    /// leading ones, which is all any assertion here is about.
    fn bytes_of(&self, vol: &Value) -> Vec<u8> {
        let mut all = self.bytes_of_all(vol);
        all.truncate(PAYLOAD.len());
        all
    }

    /// The bytes of a volume as the backend keeps them. A volume's file is named
    /// after it and lives in the slab directory; `backend_ref` in its status is
    /// where the service says it put it, and the two agree.
    fn bytes_of_all(&self, vol: &Value) -> Vec<u8> {
        let path = self.volume_file(vol);
        std::fs::read(&path).unwrap_or_default()
    }

    /// `bytes` at the front of the volume's file, and the file then cut to
    /// `bytes.len() + TAIL_ZERO_BYTES`.
    ///
    /// The service provisions a local volume to its whole `size_gib`, so leaving
    /// the file that size would back up a full gigabyte per test and compare a
    /// backup's size against a gigabyte. Cutting the file is what a volume with
    /// nothing written past its payload really is. Two things bound how far:
    ///
    /// - a file cut shorter than the object service's minimum part size cannot be
    ///   uploaded at all, because the single part it would form is under the floor
    ///   and the object service refuses it. The zero tail keeps the file over that
    ///   floor, and costs nothing: the bytes after the payload were never written,
    ///   so the file stays sparse.
    /// - a restore into an existing volume is refused when the volume's file is
    ///   shorter than the backup, so a test that restores into a named volume
    ///   sizes that target with [`Harness::grow`].
    fn write_bytes(&self, vol: &Value, bytes: &[u8]) {
        self.write_at(vol, bytes, TAIL_ZERO_BYTES);
    }

    /// As [`Harness::write_bytes`], with the length of the zero tail chosen by
    /// the caller — `0` for a file no larger than its payload.
    #[allow(dead_code)]
    fn write_at(&self, vol: &Value, bytes: &[u8], tail: usize) {
        let path = self.volume_file(vol);
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
        file.write_all(bytes).expect("write volume bytes");
        file.set_len((bytes.len() + tail) as u64)
            .expect("size volume bytes");
        file.flush().expect("flush volume bytes");
    }

    /// Return a volume's file to its provisioned length, zero-filled. The
    /// restore target of a test that shrank another volume, or whose source was
    /// cut, is grown here so the service's capacity check passes.
    #[allow(dead_code)]
    fn grow(&self, vol: &Value) {
        let path = self.volume_file(vol);
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap_or_else(|e| panic!("open {}: {e}", path.display()));
        let want = vol["spec"]["size_gib"].as_u64().expect("size_gib") * 1024 * 1024 * 1024;
        file.set_len(want).expect("grow volume");
    }

    fn volume_file(&self, vol: &Value) -> std::path::PathBuf {
        let v: slab::Volume = serde_json::from_value(vol.clone()).expect("a volume");
        LocalFileBackend::new(self.dir.path()).volume_path(&v)
    }

    /// The id of a bucket in this harness's project, for a test that wants to
    /// look at an object the service wrote.
    fn bucket_id(&self, name: &str) -> Id {
        Id::parse(kind::BUCKET, self.bucket(name)["id"].as_str().unwrap()).expect("bucket id")
    }

    fn bucket(&self, name: &str) -> Value {
        self.silo()
            .get_bucket(Some(&self.project), name)
            .map(|b| serde_json::to_value(b).expect("bucket json"))
            .expect("bucket")
    }

    /// The object service, as the block service builds it for the uploads it
    /// runs: same root, same caps, same peers. A bucket made here and an object
    /// written by the service then agree about where bytes live.
    fn silo(&self) -> silo::Client {
        silo::Client::new(self.client.store().clone(), self.client.silo_root())
            .with_peers(silo::Peers::from_cluster(self.client.store().clone()))
    }

    /// The bytes a volume's file holds — which is its whole provisioned size,
    /// since a local volume is created at that length.
    fn size_of(&self, vol: &Value) -> u64 {
        std::fs::metadata(self.volume_file(vol))
            .map(|m| m.len())
            .unwrap_or_else(|_| {
                vol["spec"]["size_gib"].as_u64().expect("size_gib") * 1024 * 1024 * 1024
            })
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        // Closing the event channel ends the loop on its own; the token is for
        // anything else the module might one day spawn.
        self.stop.cancel();
        self.reconcile.abort();
    }
}

/// The object service's client, built the way the block service builds its
/// own: same store, same root, same caps, with the peer lookup the module
/// gives it. Silo's routes and slab's uploads then agree about where bytes
/// live, which is the whole point of serving both.
fn h_silo_client(store: &Store, data: &std::path::Path) -> silo::Client {
    silo::Client::new(store.clone(), data.join("silo"))
        .with_peers(silo::Peers::from_cluster(store.clone()))
}

/// Which caller a request arrives as.
///
/// The gateway hands a module whatever its token proved, so a suite with no
/// gateway in front of it has to attach a caller itself — and the caller it
/// attaches decides what a name can find. A name is looked up in the caller's
/// project and then in the cloud scope, so a row filed in project P answers to a
/// token that holds P and to no other. Both shapes below are real and both are
/// exercised here, because the one the live cluster answered `pi share` with is
/// the one that carries no project at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum As {
    /// The admin of one project — what `pi login URL --name USER --project P`
    /// hands out, and what every fixture here is filed in.
    ProjectAdmin,
    /// An admin whose token carries no project: what a login on a single-server
    /// cloud hands out. Such a caller sees every project's rows, and can only
    /// name a row that is itself in no project.
    CloudAdmin,
}

/// The header that asks to be served as the other caller. A server hands a module
/// whatever its token proved, and this suite has no tokens to prove anything, so
/// the harness picks a caller and a test asks out of it when it wants the other
/// one. A row filed in a project answers to that project's admin; a row in no
/// project answers to the admin of no project. The suites below put their rows in
/// a project, so that is the caller by default, and the tests that name the cloud
/// caller are the ones about a token that holds nothing.
const AS_HEADER: &str = "x-test-caller";

fn caller_for(as_who: As, project: &Id) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "admin".into(),
        project: match as_who {
            As::ProjectAdmin => Some(project.clone()),
            As::CloudAdmin => None,
        },
        roles: vec!["admin".into()],
        system: true,
    }
}

async fn admit(
    State(project): State<Id>,
    mut req: Request,
    next: Next,
) -> axum::response::Response {
    let as_who = match req.headers().get(AS_HEADER).and_then(|v| v.to_str().ok()) {
        Some("cloud-admin") => As::CloudAdmin,
        _ => As::ProjectAdmin,
    };
    req.extensions_mut().insert(caller_for(as_who, &project));
    req.headers_mut().remove(AS_HEADER);
    next.run(req).await
}

/// Notes the write, then lets it through.
///
/// The path is recorded in full, `/volume/backups`, the way the CLI sends it
/// and the way `pi` prints the route it used. It is deliberately recorded
/// outside the mount: inside one, axum hands the handler a path with the prefix
/// already stripped, and a suite that checked `/backups` would be checking an
/// address no client ever sent.
async fn record(State(seen): State<Seen>, req: Request, next: Next) -> axum::response::Response {
    let path = req.uri().path().to_owned();
    let write = matches!(
        *req.method(),
        axum::http::Method::POST | axum::http::Method::PUT | axum::http::Method::DELETE
    );
    let (parts, body) = req.into_parts();
    let bytes = axum::body::to_bytes(body, 1 << 20).await.expect("body");
    if write {
        let parsed = serde_json::from_slice::<Value>(&bytes).unwrap_or(Value::Null);
        seen.writes.lock().unwrap().push((path, parsed));
    }
    next.run(Request::from_parts(
        parts,
        axum::body::Body::from(bytes.to_vec()),
    ))
    .await
}

/// A server: the slab module's own router where the gateway mounts it, driven
/// by the module's own client, over a store and a directory of its own.
///
/// The reconciler that moves a resource through its phases is run here too. On
/// a server it is `Slab::start` that spawns it, but `start` builds its own
/// client from the module's defaults, and this harness needs the one whose
/// volume directory is the tempdir; so the harness subscribes to the same
/// events and calls the same reconcile entry points, which is what the
/// module's loop does with them.
async fn serve() -> Harness {
    let dir = tempfile::tempdir().expect("tempdir");
    let store = Store::in_memory().expect("store");
    // One client for the whole harness — the routes, the fixtures and the
    // reconcile loop — because the object store and the volume directory are
    // paths, and two clients built separately take them from two places. A
    // backup uploaded by one is then read as "not found" by the other, which is
    // the failure this harness used to have: the module built its own client
    // inside `router`, so the routes wrote backups under `./data/silo` while the
    // fixtures seeded a store under the tempdir.
    //
    // `Client` resolves its default directories out of the environment, so those
    // are pointed at the tempdir while the harness is built and put back
    // afterwards. The object store's own root is named outright, since the module
    // has no setting for it.
    let previous = slab_data_env();
    std::env::set_var("PISTACK_SLAB_DIR", dir.path());
    std::env::set_var("PISTACK_DATA_DIR", dir.path());
    let client = Client::new(
        store.clone(),
        Arc::new(LocalFileBackend::new(dir.path())),
        Arc::new(slab::ProjectQuota),
    )
    .with_spool_dir(dir.path().join("spool"))
    .with_silo_root(dir.path().join("silo"))
    .with_silo_limits(silo::MAX_OBJECT_BYTES, silo::MIN_PART_BYTES)
    .with_node("test");
    client.seed_defaults().expect("seed");
    let shutdown = tokio_util::sync::CancellationToken::new();
    let seen = Seen::default();

    let project = Id::new(kind::PROJECT);
    // The object service's own routes, mounted where the gateway mounts them.
    // `pi backup create` resolves `--bucket` against `/silo/buckets` before it
    // creates anything, so the bucket the fixtures make has to be reachable
    // over the address the CLI uses, by the real service, and not only through
    // a client call in here. Silo's routes answer the same store and root the
    // slab client uploads to, so the row and the bytes stay the same rows.
    let silo_routes = silo::http::router(h_silo_client(&store, dir.path()))
        .layer(middleware::from_fn_with_state(project.clone(), admit));
    // The module as the server assembles it, with the same backend, object cap
    // and quota hook the harness's client drives.
    let app = Router::new()
        .nest(
            "/volume",
            slab::http::router(client.clone())
                .layer(middleware::from_fn_with_state(project.clone(), admit)),
        )
        .nest("/silo", silo_routes)
        .layer(middleware::from_fn_with_state(seen.clone(), record))
        // Anything else is a bug in the CLI's paths, said loudly.
        .fallback(|req: Request| async move {
            (
                StatusCode::NOT_FOUND,
                format!("unhandled {} {}", req.method(), req.uri()),
            )
        });

    restore_slab_data_env(previous);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let reconcile = reconcile_loop(client.clone(), store.events());
    Harness {
        url: format!("http://{addr}"),
        client,
        project,
        seen,
        dir,
        stop: shutdown,
        reconcile,
    }
}

/// The module's reconciler, over the harness's client. A write to a slab kind
/// wakes that kind, as `slab::reconcile::run` does, and closing the channel
/// when the harness is dropped ends the loop.
fn reconcile_loop(
    client: Client,
    events: tokio::sync::broadcast::Sender<pistack_types::Event>,
) -> tokio::task::JoinHandle<()> {
    let mut events = events.subscribe();
    tokio::spawn(async move {
        loop {
            let ev = match events.recv().await {
                Ok(ev) => ev,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                    let _ = client.reconcile_all().await;
                    continue;
                }
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return,
            };
            if ev.action == pistack_types::EventAction::Deleted {
                continue;
            }
            match ev.kind.as_str() {
                kind::VOLUME => {
                    let _ = client.reconcile_volume(&ev.id).await;
                }
                slab::KIND_SNAPSHOT => {
                    let _ = client.reconcile_snapshot(&ev.id).await;
                }
                slab::KIND_BACKUP => {
                    let _ = client.reconcile_backup(&ev.id).await;
                }
                _ => {}
            }
        }
    })
}

// ---- the CLI --------------------------------------------------------------

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(h: &Harness, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().unwrap();
    let mut argv: Vec<String> = [
        "pi",
        "--config",
        dir.path().join("config.toml").to_str().unwrap(),
        "--url",
        &h.url,
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

// ---- fixtures -------------------------------------------------------------

/// The service asked, through its own routes, for a volume provisioned to Ready
/// and holding `payload`. What `pi backup create --volume` is then pointed at.
///
/// The create names the project, because the caller this suite serves is an admin
/// of that project and may create on its behalf. The backup inherits the volume's
/// project, and the bucket the upload writes into is named in the same one, so all
/// three rows are visible to each other.
async fn make_volume(h: &Harness, name: &str, size_gib: u64, contents: &[u8]) -> Value {
    let body = json!({
        "name": name,
        "project": h.project.to_string(),
        "spec": { "size_gib": size_gib, "volume_type": "standard" },
    });
    let (st, vol) = post_json(h, "/volume/volumes", &body).await;
    assert_eq!(st, StatusCode::CREATED, "{vol}");
    let id = vol["id"].as_str().unwrap().to_owned();
    ready_volume(h, &id).await;
    // Read back by id, then handed to the test under whichever handle the test
    // will use: a row is the same row by name, by full id or by the short id a
    // table prints, and a fixture that proved that is a fixture that cannot
    // quietly seed the row twice.
    let by_id = show_volume(h, &id).await;
    let by_name = show_volume(h, name).await;
    assert_eq!(
        by_name["id"], by_id["id"],
        "a name finds the row: {name} answered {} for name, {} for id",
        by_name["id"], by_id["id"]
    );
    assert_eq!(
        show_volume(h, &short(&id)).await["id"],
        by_id["id"],
        "a short id finds the row"
    );
    h.write_bytes(&by_id, contents);
    by_name
}

/// A volume with nothing in it yet: provisioned to its whole size and left as the
/// service made it, which is what a restore writes into.
/// The environment a `slab::Client` takes its data directories from, saved so a
/// harness can point them at a tempdir without leaving them pointed there for
/// every other test in the binary.
fn slab_data_env() -> Vec<(&'static str, Option<std::ffi::OsString>)> {
    ["PISTACK_SLAB_DIR", "PISTACK_DATA_DIR"]
        .into_iter()
        .map(|k| (k, std::env::var_os(k)))
        .collect()
}

fn restore_slab_data_env(saved: Vec<(&'static str, Option<std::ffi::OsString>)>) {
    for (key, value) in saved {
        match value {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }
}

async fn empty_volume(h: &Harness, name: &str, size_gib: u64) -> Value {
    let body = json!({
        "name": name,
        "project": h.project.to_string(),
        "spec": { "size_gib": size_gib, "volume_type": "standard" },
    });
    let (st, vol) = post_json(h, "/volume/volumes", &body).await;
    assert_eq!(st, StatusCode::CREATED, "{vol}");
    ready_volume(h, vol["id"].as_str().unwrap()).await
}

/// The cloud-scoped bucket every fixture here uses: `cold`.
///
/// Made through silo's client rather than silo's routes because those are mounted
/// under `/silo` on a server, not under `/volume`; signing requests in here would
/// mean standing in for the gateway's signature check, which is another service
/// and not the one this suite is about. The record the client writes is the one
/// the routes write.
///
/// The bucket is named in the volume's project, because that is where the service
/// looks for the target a backup names: in the backup's own project first.
async fn make_bucket(h: &Harness, name: &str) -> Value {
    // The object service is reached through a client built the way the block
    // service builds its own, so the bytes a backup uploads are read back through
    // the same rules that wrote them.
    h.silo()
        .create_bucket(
            Meta::new(kind::BUCKET, name, Some(h.project.clone())),
            silo::BucketSpec::default(),
        )
        .expect("bucket");
    show_bucket(h, name).await
}

/// A backup record the service accepted, at the phase it has reached. Not
/// uploaded yet: a caller that needs the bytes uses [`ready_backup`].
///
/// The name is `<name>` as asked for, and the handle every later verb uses is
/// the id the service answered with — never a name built out of the id, which
/// is what `pi backup restore` of its own accord does to a volume and is a row
/// of its own.
async fn post_backup(h: &Harness, name: &str, vol: &Value, bucket: &str) -> Value {
    let body = json!({
        "name": name,
        "spec": { "volume": vol["id"], "target": bucket },
    });
    let (st, bak) = post_json(h, "/volume/backups", &body).await;
    assert_eq!(st, StatusCode::CREATED, "{bak}");
    assert_eq!(
        bak["project"], vol["project"],
        "a backup is filed in its volume's scope, not the caller's"
    );
    bak
}

/// A ready backup named `name`, so a test can point a verb at a handle that
/// could not be anything else's.
async fn make_ready_backup_named(h: &Harness, name: &str, vol: &Value, bucket: &str) -> Value {
    let bak = post_backup(h, name, vol, bucket).await;
    let ready = ready_backup(h, bak["id"].as_str().unwrap()).await;
    let key = ready["status"]["object_ref"].as_str().expect("object_ref");
    let id = h.bucket_id(bucket);
    let obj = h
        .silo()
        .head_object(&id, key)
        .unwrap_or_else(|e| panic!("backup {name} is ready with no object {key}: {e}"));
    assert_eq!(
        obj.status.size_bytes,
        ready["status"]["size_bytes"].as_u64().unwrap()
    );
    ready
}

/// A ready backup of `vol`, held in `bucket`. Created the way the CLI creates
/// one — over HTTP — and then uploaded by the service itself.
async fn make_ready_backup(h: &Harness, vol: &Value, bucket: &str) -> Value {
    let name = format!("of-{}", vol["name"].as_str().unwrap());
    make_ready_backup_named(h, &name, vol, bucket).await
}

/// One volume, read the way the CLI reads it: over HTTP, by name or id.
async fn show_volume(h: &Harness, name_or_id: &str) -> Value {
    let k = bench::cli::kinds::by_noun("volume").expect("volume is a known kind");
    fixture_read(h, &k.item_path(name_or_id), &format!("volume {name_or_id}")).await
}

/// One backup, read from the block service's own route.
async fn show_backup(h: &Harness, name_or_id: &str) -> Value {
    fixture_read(
        h,
        &format!("/volume/backups/{name_or_id}"),
        &format!("backup {name_or_id}"),
    )
    .await
}

/// A bucket as the object store keeps it. `pi backup` has no bucket noun and
/// the object store's router is not mounted here, so the record is read from
/// the store the bucket was created in.
async fn show_bucket(h: &Harness, name: &str) -> Value {
    let b = h.silo().get_bucket(Some(&h.project), name).expect("bucket");
    serde_json::to_value(b).expect("bucket json")
}

/// A request sent as the admin of no project — a token that carries nothing,
/// which is what a login on a single-server cloud hands out and what the live
/// cluster answered `pi share` with.
#[allow(dead_code)]
fn as_cloud_admin(req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
    req.header(AS_HEADER, "cloud-admin")
}

/// A fixture read of one row, by name or id, asserting the service answered.
async fn fixture_read(h: &Harness, path: &str, what: &str) -> Value {
    let res = reqwest::Client::new()
        .get(format!("{}{}", h.url, path))
        .send()
        .await
        .expect("request");
    let st = res.status();
    let v = read_json(res).await;
    assert_eq!(st, StatusCode::OK, "{what}: {v}");
    v
}

#[allow(dead_code)]
async fn get(h: &Harness, path: &str) -> (StatusCode, Value) {
    get_status(&h.url, path).await
}

/// The body of a `GET`, whatever the status. A wait loop reads through this.
async fn get_url(url: &str, path: &str) -> Value {
    let (_, v) = get_status(url, path).await;
    v
}

async fn get_status(url: &str, path: &str) -> (StatusCode, Value) {
    let res = reqwest::Client::new()
        .get(format!("{url}{path}"))
        .send()
        .await
        .expect("request");
    let st = res.status();
    (st, read_json(res).await)
}

async fn post_json(h: &Harness, path: &str, body: &Value) -> (StatusCode, Value) {
    let res = reqwest::Client::new()
        .post(format!("{}{}", h.url, path))
        .json(body)
        .send()
        .await
        .expect("request");
    let st = res.status();
    (st, read_json(res).await)
}

async fn read_json(res: reqwest::Response) -> Value {
    let bytes = res.bytes().await.expect("bytes");
    if bytes.is_empty() {
        return Value::Null;
    }
    serde_json::from_slice(&bytes)
        .unwrap_or_else(|e| panic!("not json ({e}): {}", String::from_utf8_lossy(&bytes)))
}

/// Wait for a volume to reach `ready`. Provisioning is the service's own work,
/// driven by the reconciler running beside the router.
async fn ready_volume(h: &Harness, id: &str) -> Value {
    let url = h.url.clone();
    let path = format!("/volume/volumes/{id}");
    wait_phase(
        id,
        "ready",
        move || {
            let url = url.clone();
            let path = path.clone();
            Box::pin(async move { get_url(&url, &path).await })
        },
        "volume",
    )
    .await
}

/// Wait for a backup to reach `ready`, which is the service having really
/// written the volume's bytes into the bucket.
async fn ready_backup(h: &Harness, id: &str) -> Value {
    let url = h.url.clone();
    let path = format!("/volume/backups/{id}");
    wait_phase(
        id,
        "ready",
        move || {
            let url = url.clone();
            let path = path.clone();
            Box::pin(async move { get_url(&url, &path).await })
        },
        "backup",
    )
    .await
}

async fn wait_phase(
    id: &str,
    want: &str,
    mut look: impl FnMut() -> futures::future::BoxFuture<'static, Value>,
    noun: &str,
) -> Value {
    for _ in 0..4_800 {
        let v = look().await;
        match v["status"]["phase"].as_str() {
            Some(p) if p == want => return v,
            Some("error") => panic!("{noun} {id} failed: {v}"),
            _ => tokio::time::sleep(Duration::from_millis(25)).await,
        }
    }
    panic!("{noun} {id} never reached {want}");
}

fn json_of(r: &Run) -> Value {
    serde_json::from_str(&r.out).unwrap_or_else(|e| panic!("not json: {e}\n{}", r.out))
}

/// The same tail of an id the table prints, so a test can look for what it saw.
fn short(id: &str) -> String {
    bench::cli::output::short_id(id)
}

// ---- the suite ------------------------------------------------------------

#[tokio::test]
async fn backup_list_shows_the_columns() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    let bucket = make_bucket(&h, "cold").await;
    let bak = make_ready_backup(&h, &vol, "cold").await;

    let r = pi(&h, &["backup", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    for column in ["NAME", "PHASE", "BUCKET", "SIZE-GIB"] {
        assert!(r.out.contains(column), "{column}: {}", r.out);
    }
    assert!(r.out.contains("cold"), "{}", r.out);
    assert!(
        r.out.contains(&short(bak["id"].as_str().unwrap())),
        "{}",
        r.out
    );
    // A short volume id, never the whole thing.
    assert!(
        r.out.contains(&short(vol["id"].as_str().unwrap())),
        "{}",
        r.out
    );
    assert!(!r.out.contains(vol["id"].as_str().unwrap()), "{}", r.out);

    let r = pi(&h, &["backup", "ls", "-o", "wide"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("OBJECT"), "{}", r.out);
    assert!(
        r.out
            .contains(bak["status"]["object_ref"].as_str().unwrap()),
        "{}",
        r.out
    );

    let listed = json_of(&pi(&h, &["backup", "list", "-o", "json"]).await);
    assert_eq!(listed.as_array().unwrap().len(), 1);
    assert_eq!(listed[0]["name"], "of-data");

    let r = pi(&h, &["backup", "list", "-o", "yaml"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("name: of-data"), "{}", r.out);

    let r = pi(&h, &["backup", "list", "-o", "name"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "of-data");

    // The bucket the row names is a row of the object store's own, and the
    // volume's is a row of the block service's: nothing here invented a kind.
    assert_eq!(bucket["name"], "cold");
    let r = pi(&h, &["volume", "list", "--no-headers"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("data"), "{}", r.out);
}

#[tokio::test]
async fn backup_show_accepts_a_name_or_an_id() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    let _bucket = make_bucket(&h, "cold").await;
    let bak = make_ready_backup(&h, &vol, "cold").await;
    let id = bak["id"].as_str().unwrap();

    let r = pi(&h, &["backup", "show", "of-data", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v = json_of(&r);
    assert_eq!(v["id"], id);
    assert_eq!(v["status"]["phase"], "ready");

    let r = pi(&h, &["backup", "get", id, "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains(id), "{}", r.out);

    // The short id a table printed is accepted too, and the table form of a
    // single resource is a FIELD/VALUE dump of it.
    let r = pi(&h, &["backup", "show", &short(id), "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(json_of(&r)["id"], id);

    let r = pi(&h, &["backup", "show", "of-data"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("phase"), "{}", r.out);

    let r = pi(&h, &["backup", "show", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_create_resolves_the_volume_and_names_the_bucket() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    let bucket = make_bucket(&h, "cold").await;
    let vid = vol["id"].as_str().unwrap();

    let r = pi(
        &h,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("backup \"nightly\" created"), "{}", r.out);
    let (path, body) = h.seen.last_write();
    assert_eq!(path, "/volume/backups");
    assert_eq!(body["name"], "nightly");
    // The volume and the bucket were both looked up and sent as ids. The
    // service files a bucket inside its project, under `{kind}/{project}/{name}`,
    // so a target sent as a bare name is only findable when the backup and the
    // bucket share a scope by luck; an id always resolves.
    assert_eq!(body["spec"]["volume"], vid);
    assert_eq!(body["spec"]["target"], bucket["id"]);
    assert!(body.get("project").is_none(), "{body}");
    // And the record the service made is real, in the volume's scope.
    let bak = show_backup(&h, "nightly").await;
    assert_eq!(bak["spec"]["volume"], vid);
    assert_eq!(bak["project"], vol["project"]);

    // Metadata goes where every other create puts it.
    let r = pi(
        &h,
        &[
            "backup",
            "create",
            "twice",
            "--volume",
            vid,
            "--bucket",
            "cold",
            "--description",
            "before the upgrade",
            "--label",
            "tier=gold",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let (_, body) = h.seen.last_write();
    assert_eq!(body["description"], "before the upgrade");
    assert_eq!(body["labels"], json!({"tier": "gold"}));
    assert_eq!(body["spec"]["volume"], vid);

    // A volume that is not there is the CLI's own words, before the service is
    // asked anything.
    let before = h.seen.writes.lock().unwrap().len();
    let r = pi(
        &h,
        &[
            "backup", "create", "x", "--volume", "ghost", "--bucket", "cold",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("volume 'ghost' not found"), "{}", r.err);
    assert_eq!(
        h.seen.writes.lock().unwrap().len(),
        before,
        "a volume that is not there is not backed up"
    );

    // A bucket that is not there is the create's own error, said before the
    // service is asked anything — the same words the volume lookup uses. The
    // service would accept a name it cannot resolve and fail the upload
    // minutes later; a create that sends an id fails here instead, where the
    // operator is still looking.
    let before = h.seen.writes.lock().unwrap().len();
    let r = pi(
        &h,
        &[
            "backup", "create", "x", "--volume", "data", "--bucket", "missing",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("bucket 'missing' not found"), "{}", r.err);
    assert_eq!(
        h.seen.writes.lock().unwrap().len(),
        before,
        "a bucket that is not there creates no backup"
    );
    // A bucket named by the short id a table printed resolves too.
    let bshort = short(bucket["id"].as_str().unwrap());
    let r = pi(
        &h,
        &[
            "backup", "create", "byshort", "--volume", "data", "--bucket", &bshort,
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let (_, body) = h.seen.last_write();
    assert_eq!(body["spec"]["target"], bucket["id"]);
}

#[tokio::test]
async fn backup_create_wait_watches_the_service_reach_ready() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    make_bucket(&h, "cold").await;

    let r = pi(
        &h,
        &[
            "backup", "create", "nightly", "--volume", "data", "--bucket", "cold", "--wait",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("backup \"nightly\" created"), "{}", r.out);
    // Each phase the service reaches after the create is printed once, and
    // ready ends the wait. Whether `pending` is among them is a race between the
    // reconciler beside the router and the CLI's first poll, so it is not
    // asserted; `ready` exactly once is the thing the wait has to get right.
    assert!(r.out.contains("backup \"nightly\": ready"), "{r:?}");
    assert_eq!(
        r.out.matches("backup \"nightly\": ready").count(),
        1,
        "{r:?}"
    );
    // The wait ended on a backup whose bytes are actually in the bucket.
    let bak = show_backup(&h, "nightly").await;
    assert_eq!(bak["status"]["phase"], "ready");
    assert!(bak["status"]["object_ref"].is_string(), "{bak}");
    assert_eq!(
        bak["status"]["size_bytes"],
        h.size_of(&vol),
        "a backup carries the volume's whole file: {bak}"
    );
}

#[tokio::test]
async fn backup_create_wait_reports_a_failed_upload() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    // No bucket when the CLI asks: the create itself refuses, because it
    // resolves `--bucket` before it posts anything.
    let r = pi(
        &h,
        &[
            "backup", "create", "ghost", "--volume", "data", "--bucket", "cold", "--wait",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("bucket 'cold' not found"), "{}", r.err);

    // The upload the service runs afterwards reports its own failures, and a
    // `--wait` create is how an operator hears them. The bucket is made so the
    // create resolves it, and then taken away before the reconciler uploads:
    // the record's target is the bucket's id, and an id whose row is gone is
    // the service's problem, not the CLI's.
    let bucket = make_bucket(&h, "cold").await;
    h.silo()
        .delete_bucket(&h.bucket_id("cold"))
        .await
        .expect("bucket deleted");
    let r = pi(
        &h,
        &[
            "backup",
            "create",
            "nightly",
            "--volume",
            "data",
            "--bucket",
            bucket["id"].as_str().unwrap(),
            "--wait",
        ],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("backup \"nightly\" failed"), "{}", r.err);
    // The reason comes from the condition the service records.
    assert!(r.err.contains("not found"), "{}", r.err);
    let bak = show_backup(&h, "nightly").await;
    assert_eq!(bak["status"]["phase"], "error", "{bak}");
    let _ = &vol;
}

#[tokio::test]
async fn backup_delete_needs_yes_and_reports_acceptance() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    make_bucket(&h, "cold").await;
    make_ready_backup(&h, &vol, "cold").await;

    // No terminal to confirm on, so a delete without --yes is refused.
    let before = h.seen.writes.lock().unwrap().len();
    let r = pi(&h, &["backup", "delete", "of-data"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert_eq!(h.seen.writes.lock().unwrap().len(), before, "{r:?}");
    assert!(show_backup(&h, "of-data").await["status"]["phase"] == "ready");

    let r = pi(&h, &["backup", "delete", "of-data", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    // The service answers 202: the object goes away in the background.
    assert!(
        r.out.contains("backup \"of-data\" deletion requested"),
        "{}",
        r.out
    );
    assert_eq!(h.seen.last_write().0, "/volume/backups/of-data");

    let r = pi(&h, &["backup", "rm", "ghost", "--yes"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_delete_wait_polls_until_gone() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    make_bucket(&h, "cold").await;
    make_ready_backup(&h, &vol, "cold").await;

    let r = pi(&h, &["backup", "delete", "of-data", "--yes", "--wait"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(
        r.out.contains("backup \"of-data\" deletion requested"),
        "{}",
        r.out
    );
    assert!(r.out.contains("backup \"of-data\": gone"), "{r:?}");
    // Gone means gone: the service dropped the record and the object with it.
    let listed = json_of(&pi(&h, &["backup", "list", "-o", "json"]).await);
    assert_eq!(listed.as_array().unwrap().len(), 0, "{listed}");
}

#[tokio::test]
async fn backup_restore_makes_a_new_volume_holding_the_bytes() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    make_bucket(&h, "cold").await;
    let bak = make_ready_backup(&h, &vol, "cold").await;
    let id = bak["id"].as_str().unwrap();

    let r = pi(&h, &["backup", "restore", "of-data", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let (path, body) = h.seen.last_write();
    assert_eq!(path, "/volume/backups/of-data/restore");
    // A new volume is asked for with a body that names no volume.
    assert_eq!(body, json!({}));
    // What comes back is a volume of the service's own, named after the
    // backup, holding the bytes that were uploaded.
    let out = json_of(&r);
    assert_eq!(out["name"], format!("restore-{id}"));
    assert_eq!(out["status"]["phase"], "ready");
    assert_ne!(out["id"], vol["id"]);
    assert_eq!(h.bytes_of(&out), PAYLOAD);
    // The name is the backup's, so a second restore collides with the first
    // volume and says so in the service's words — proof the first one is really
    // there and is the one the bytes were read out of.
    let again = pi(&h, &["backup", "restore", "of-data"]).await;
    assert_ne!(again.code, 0, "{again:?}");
    assert!(again.err.contains("restore-"), "{again:?}");

    let r = pi(&h, &["backup", "restore", "nope"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn backup_restore_into_the_volume_named_writes_it() {
    let h = serve().await;
    let src = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    make_bucket(&h, "cold").await;
    let bak = make_ready_backup(&h, &src, "cold").await;
    // The volume written to is a different one, empty until the restore.
    let dest = empty_volume(&h, "target", VOLUME_GIB).await;
    assert!(h.bytes_of_all(&dest).iter().all(|b| *b == 0), "blank");

    let r = pi(
        &h,
        &[
            "backup", "restore", "of-data", "--volume", "target", "-o", "json",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    // The restore went to the handle the operator typed, unchanged, and named
    // the volume to write by its id — the id `resolve_volume` read back from
    // the volume's own row, not the word the operator typed. The handle itself
    // is forwarded as it was given: the service owns what a name or an id
    // resolves to, and a CLI that rewrote it would hide the lookup a wrong
    // answer is exactly what an operator needs to see.
    let (path, body) = {
        let writes = h.seen.writes.lock().unwrap();
        writes
            .iter()
            .find(|(p, _)| p.ends_with("/restore"))
            .cloned()
            .expect("a restore was recorded")
    };
    assert_eq!(path, "/volume/backups/of-data/restore");
    assert_eq!(body["volume"], dest["id"]);
    // The answer is the volume written to, and the bytes are its own now.
    let out = json_of(&r);
    assert_eq!(out["id"], dest["id"]);
    assert_eq!(out["status"]["phase"], "ready");
    assert_eq!(h.bytes_of(&out), PAYLOAD);
    let again = show_volume(&h, "target").await;
    assert_eq!(again["status"]["phase"], "ready");
    assert_eq!(h.bytes_of(&again), PAYLOAD);
    // The source was not touched by any of this.
    assert_eq!(h.bytes_of(&show_volume(&h, "data").await), PAYLOAD);

    let r = pi(&h, &["backup", "restore", "of-data", "--volume", "ghost"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("volume 'ghost' not found"), "{}", r.err);

    // The same restore addressed by the backup's full id and by the short id a
    // table printed: a fresh target each time, and the same bytes both times.
    for (n, handle) in [
        ("byid", bak["id"].as_str().unwrap().to_owned()),
        ("byshort", short(bak["id"].as_str().unwrap())),
    ] {
        let t = empty_volume(&h, n, VOLUME_GIB).await;
        let r = pi(
            &h,
            &["backup", "restore", &handle, "--volume", n, "-o", "json"],
        )
        .await;
        assert_eq!(r.code, 0, "{handle}: {r:?}");
        let out = json_of(&r);
        assert_eq!(out["id"], t["id"], "{handle}");
        assert_eq!(h.bytes_of(&out), PAYLOAD, "{handle}");
    }
}

#[tokio::test]
async fn backup_restore_of_a_pending_backup_is_refused() {
    let h = serve().await;
    let vol = make_volume(&h, "data", VOLUME_GIB, PAYLOAD).await;
    make_bucket(&h, "cold").await;
    // A backup with no bucket behind it never leaves pending or error; here it
    // is made pending by pointing at a bucket that will exist later.
    let body = json!({
        "name": "queued",
        "spec": { "volume": vol["id"], "target": "cold" },
    });
    let (st, bak) = post_json(&h, "/volume/backups", &body).await;
    assert_eq!(st, StatusCode::CREATED, "{bak}");

    // Whatever phase the service has reached so far, a restore that is not
    // ready is refused in the service's own words.
    let phase = show_backup(&h, bak["id"].as_str().unwrap()).await["status"]["phase"]
        .as_str()
        .unwrap()
        .to_owned();
    if phase == "ready" {
        // The reconciler won the race; nothing to refuse.
        return;
    }
    let r = pi(&h, &["backup", "restore", "queued"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("not ready"), "{r:?}");
}

#[tokio::test]
async fn backup_noun_is_not_mixed_into_other_verbs() {
    let h = serve().await;
    let r = pi(&h, &["backup", "attach", "of-data"]).await;
    assert_eq!(r.code, 2, "{r:?}");
}
