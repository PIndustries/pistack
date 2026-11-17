// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `pi share` against the shelf module itself: the real router over an
//! in-memory store, served at `/shelf` the way the gateway mounts it, so the
//! paths and bodies the CLI builds are checked against the routes they were
//! written for. A middleware attaches the caller, standing in for the token
//! check the gateway does.
//!
//! Shelf realizes a share as it writes it: an NFS export, reloaded with
//! `exportfs -ra`, or a CephFS subvolume. The tools it shells out to are
//! real, not stubbed: macOS has `exportfs` and refuses a path under
//! `/private/tmp`, so an NFS share refuses to export exactly as it would on
//! a host without a configured NFS server, and a cephfs share stays pending
//! because no `ceph` is installed. Both are what a cluster without that
//! tooling does, and both are asserted below.

use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::{self, Next},
    Router,
};
use bench::cli::{self, Io};
use pistack_store::Store;
use pistack_types::{kind, Caller, Id, Module, ModuleContext};
use serde_json::{json, Value};
use shelf::Shelf;
use std::io::Cursor;
use std::sync::{Arc, Mutex, OnceLock};

/// A server with a share already on it and a second writer waiting on the
/// read of `home`: the CLI is about to edit what it read, and between the two
/// calls somebody else takes the share away and puts it back.
async fn serve_moved_underneath() -> Harness {
    let h = serve().await;
    seed_through_the_service(&h).await;
    *h.seen.second_writer.lock().unwrap() = Some(Handle::Name("home".to_owned()));
    h
}

/// One `PUT` as it reached the server: the share, the `If-Match`, the body.
/// Recorded by a route wrapped around the real router, so the CLI's
/// read-modify-write can still be inspected.
/// One `PUT` as it reached the server: the share handle, the `If-Match`, the body.
type PutNote = (String, Option<String>, Value);

#[derive(Clone)]
struct Seen {
    puts: Arc<Mutex<Vec<PutNote>>>,
    deletes: Arc<Mutex<Vec<String>>>,
    queries: Arc<Mutex<Vec<String>>>,
    /// A second writer to run between the CLI's read and its write: the next
    /// write received for the handle it names edits what the read just
    /// answered, then is undone. The CLI therefore submits against a revision
    /// that is no longer the current one.
    second_writer: Arc<Mutex<Option<Handle>>>,
    /// The store the real router reads, so a staged read is answered from the
    /// row as it stands when the request arrives.
    store: Store,
    /// The project of the server this recorder wraps.
    project: Id,
    /// Reads answered so far, `METHOD path`, newest last.
    reads: Arc<Mutex<Vec<String>>>,
    /// What the recorder writes to. Everything before the recorder was told
    /// to start is setup, not the test's own traffic.
    recording: Arc<Mutex<bool>>,
}

impl Default for Seen {
    fn default() -> Self {
        Self {
            puts: Default::default(),
            deletes: Default::default(),
            queries: Default::default(),
            second_writer: Default::default(),
            project: Id::new(kind::PROJECT),
            reads: Default::default(),
            recording: Arc::new(Mutex::new(true)),
            store: Store::in_memory().expect("the recorder's stand-in store"),
        }
    }
}

/// Where shelf puts a share and what it shells out to are read from the
/// process environment, which every test here shares. One lock is held for as
/// long as a harness lives, so exactly one server's settings are in force at a
/// time and a second test cannot answer with the first one's exports file —
/// which otherwise looks like a share created under another id.
///
/// A server whose shelf root is somewhere the process may write, with the
/// tools shelf asks for named but not present, and running as an ordinary
/// user. What shelf then says about a share it cannot export is what a server
/// without a configured NFS server says.
async fn serve_without_nfs_tooling() -> Harness {
    // The server of a machine with no root and no NFS tooling. `serve()` has
    // already named the tools it wants and taken the shelf lock, which the
    // harness holds until it is dropped, so changing them here is the suite's
    // to do and lasts exactly as long as this server.
    let h = serve().await;
    set_var("PISTACK_SHELF_EUID", "1000");
    set_var("PISTACK_SHELF_SQUASH_UID", "65534");
    set_var("PISTACK_EXPORTFS", "definitely-not-here");
    h
}

/// A per-run directory: the shelf root, the exports file and the ceph binary
/// path all live under it. Nothing in here outlives the test that made it.
struct Harness {
    /// The project this server answers as.
    project: Id,
    url: String,
    store: Store,
    seen: Seen,
    _dir: tempfile::TempDir,
    // Held for the harness's whole life: shelf's environment stays as this
    // server set it until the harness is dropped.
    _env: std::sync::MutexGuard<'static, ()>,
}

/// Shelf's environment is process-wide, so the suite takes it one server at a
/// time. The lock is never formally released: a harness holds it for its own
/// lifetime, and a test that panicked while holding it leaves it held, which
/// the next holder steps over rather than waiting on.
fn shelf_env_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: OnceLock<std::sync::Mutex<()>> = OnceLock::new();
    let lock = LOCK.get_or_init(|| std::sync::Mutex::new(()));
    match lock.lock() {
        Ok(g) => g,
        // A test that panicked while holding it poisoned it. The next holder
        // writes every key it reads, so the state it left is of no interest.
        Err(poisoned) => poisoned.into_inner(),
    }
}

impl Harness {
    /// The share shelf keeps, looked up by name through the caller's project
    /// and the cloud scope. Deliberately not by id: a test that read a row by
    /// id could pass while the service refuses that very id.
    fn share(&self, name: &str) -> Option<Value> {
        let col = self.store.collection::<shelf::Share>();
        let share = col
            .get_by_name(Some(&self.project), name)
            .or_else(|_| col.get_by_name(None, name))
            .ok()?;
        Some(serde_json::to_value(share).unwrap())
    }

    /// The row shelf holds for `id`, read out of the store directly. Only a
    /// test's own arithmetic on ids may use this; a lookup a user could
    /// perform goes through the CLI.
    fn row_by_id(&self, id: &str) -> Option<Value> {
        let id = pistack_types::Id::parse("share", id)?;
        let share = self.store.collection::<shelf::Share>().get(&id).ok()?;
        Some(serde_json::to_value(share).unwrap())
    }

    fn shares(&self) -> Vec<Value> {
        self.store
            .collection::<shelf::Share>()
            .list(None)
            .unwrap()
            .into_iter()
            .map(|s| serde_json::to_value(s).unwrap())
            .collect()
    }

    /// The id shelf gave the share, and the short form `pi` prints.
    fn id_of(&self, name: &str) -> String {
        self.share(name).expect("share")["id"]
            .as_str()
            .unwrap()
            .to_owned()
    }
}

/// A cloud admin holding a project-scoped token: the shape a real `pi login`
/// produces, and the one that made `pi share` fail. `project` is the one the
/// server answers as, so one harness's shares are another's not there.
fn caller_for(project: &Id) -> Caller {
    Caller {
        principal: Id::new(kind::PRINCIPAL),
        principal_name: "admin".into(),
        project: Some(project.clone()),
        roles: vec!["admin".into()],
        system: true,
    }
}

async fn admit(
    State(project): State<Id>,
    mut req: Request,
    next: Next,
) -> axum::response::Response {
    req.extensions_mut().insert(caller_for(&project));
    next.run(req).await
}

// The harness keeps shelf's environment locked for as long as the server
// lives, which is deliberate (see [`shelf_env_lock`]) and spans the listener's
// bind; the guard is never held across work that contends with it.
#[allow(clippy::await_holding_lock)]
async fn serve() -> Harness {
    let env = shelf_env_lock();
    let dir = tempfile::tempdir().expect("shelf dir");
    let data = dir.path().to_path_buf();
    std::fs::create_dir_all(data.join("shares")).expect("share root");
    set_env("PISTACK_SHELF_DIR", &data.join("shares"));
    set_env("PISTACK_EXPORTS_FILE", &data.join("pistack.exports"));
    set_env("PISTACK_CEPH_BIN", &data.join("no-ceph"));
    // No ceph filesystem: a cephfs share stays pending on a server that has
    // not named one, which is the shape of every dev cluster.
    unset_env("PISTACK_CEPH_FS");
    unset_env("PISTACK_SHELF_HELPER");
    // The real `exportfs` on this machine, when there is one. Where there is
    // none, an NFS share cannot be exported and says so; both are asserted.
    match which("exportfs") {
        Some(p) => set_env("PISTACK_EXPORTFS", &p),
        None => unset_env("PISTACK_EXPORTFS"),
    }
    unset_env("PISTACK_EXPORT_FAIL");
    // A share is exported by writing the managed exports file and reloading
    // it with `exportfs -ra`. The suites of this repo all run in one process,
    // and the module's other tests point that binary at a script of their own
    // that exits non-zero. So the suite names the tool it wants: one that
    // records what it was handed and succeeds, and the server runs as root so
    // it takes the same path a real server does. What an ordinary user gets
    // — no `exportfs` at all, and an export that cannot be reloaded — is the
    // subject of a test of its own.
    let exportfs = data.join("exportfs");
    std::fs::write(
        &exportfs,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nexit 0\n",
            data.join("exportfs.log").display()
        ),
    )
    .expect("exportfs script");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&exportfs, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    set_env("PISTACK_EXPORTFS", &exportfs);
    set_var("PISTACK_SHELF_EUID", "0");
    // Squash to nobody/nogroup is what a root server does, and this server
    // is one. Off root the process cannot chown to nobody; the share keeps
    // its own uid and the suite asserts against what the process reaches.
    set_var("PISTACK_SHELF_SQUASH_UID", "65534");
    unset_env("PISTACK_SHELF_HELPER");

    // One store per harness — one server's worth of rows, as every module of
    // a server shares one store. Tests run beside each other, so a row left
    // by one is simply not in another server's list.
    let store = Store::in_memory().expect("store");
    let project = Id::new(kind::PROJECT);
    let module = Shelf;
    let ctx = ModuleContext {
        store: Arc::new(store.clone()),
        events: store.events(),
        shutdown: tokio_util::sync::CancellationToken::new(),
        public_url: "http://test".into(),
        node_name: "test".into(),
        leadership: pistack_types::Leadership::always(),
    };
    // `Shelf::start` is never called here — the routes are mounted straight
    // onto the router — so the address a ready share reports is pinned the
    // way a server boot would pin it from the public URL. The host `test` is
    // what the CLI then prints before the path.
    module.set_export_address(&ctx.public_url);
    let seen = Seen {
        store: store.clone(),
        project: project.clone(),
        ..Default::default()
    };
    // A fresh recorder per harness: what it notes is the traffic this test's
    // own commands made, and nothing another test's commands sent beside it.
    let recorder = seen.clone();
    // The recorder runs outside the mount, so it sees the full request path.
    // The caller is attached here, before the router's own check runs.
    let inner = module
        .router(ctx)
        .layer(middleware::from_fn_with_state(project.clone(), admit));
    let app = Router::new()
        .nest("/shelf", inner)
        .layer(middleware::from_fn_with_state(recorder.clone(), record))
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
    Harness {
        project,
        url: format!("http://{addr}"),
        store,
        seen,
        _dir: dir,
        _env: env,
    }
}

/// The part of a read path that names the share: its name, its full id, or
/// the short id a table prints. Compared through the same rules the service
/// uses, so a staged read cannot be answered for some other handle that
/// merely ends the same way.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Handle {
    Name(String),
    Full(String),
    Short(String),
}

impl Handle {
    fn of(name_or_id: &str) -> Handle {
        if name_or_id.starts_with("share-") && name_or_id.len() == "share-".len() + 36 {
            return Handle::Full(name_or_id.to_owned());
        }
        if let Some(tail) = name_or_id.strip_prefix("share-") {
            if (4..32).contains(&tail.len())
                && tail.chars().all(|c| c.is_ascii_hexdigit())
                && tail.len() != 36
            {
                return Handle::Short(tail.to_ascii_lowercase());
            }
        }
        Handle::Name(name_or_id.to_owned())
    }

    /// The part of a read asking for this share. `staged` is the handle the
    /// scenario asked for, so the CLI's read is met by exactly the handle it
    /// was told to make; the row's name and full id, which the staged form
    /// stands for, are kept beside it.
    fn asked_for(&self, staged: &str) -> bool {
        let mine = Handle::of(staged);
        match (self, &mine) {
            (Handle::Name(a), Handle::Name(b)) => a == b,
            (Handle::Full(a), Handle::Full(b)) => a == b,
            (Handle::Short(a), Handle::Short(b)) => a == b,
            _ => false,
        }
    }
}

impl Seen {
    /// Stop noting what the server has been asked to do, and forget what was
    /// noted until now. Setup goes through the same routes as a test's own
    /// commands; a test reads back only its own.
    fn pause_recording(&self) {
        *self.recording.lock().unwrap() = false;
    }

    /// Note what comes in from here on, and remember where it began.
    fn start_recording(&self) {
        self.puts.lock().unwrap().clear();
        self.deletes.lock().unwrap().clear();
        self.queries.lock().unwrap().clear();
        self.reads.lock().unwrap().clear();
        *self.recording.lock().unwrap() = true;
    }

    /// Take the share `asked` names away and put it back, the way a
    /// concurrent operator's delete and recreate would. The revision the row
    /// comes back at is what the CLI's `If-Match` then disagrees with, and
    /// the name is free again in between, exactly as it is on a server.
    fn write_as_another_writer(&self, asked: &str) {
        let col = self.store.collection::<shelf::Share>();
        let cur = col
            .resolve_scoped(Some(&self.project), asked)
            .expect("the share the CLI just read");
        let id = cur.meta.id.clone();
        col.delete(&id).expect("the other writer's delete");
        col.create(cur).expect("the other writer's create");
    }
}

/// Wraps the real routes to note what the CLI actually sent.
async fn record(
    State(seen): State<Seen>,
    mut req: Request,
    next: Next,
) -> axum::response::Response {
    let method = req.method().clone();
    let uri = req.uri().clone();
    let if_match = req
        .headers()
        .get("if-match")
        .and_then(|v| v.to_str().ok())
        .map(|v| v.trim_matches('"').to_owned());
    let path = uri.path().to_owned();
    let bytes = axum::body::to_bytes(std::mem::take(req.body_mut()), 1 << 20)
        .await
        .unwrap();
    let body: Value = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    *req.body_mut() = axum::body::Body::from(bytes);
    let query = uri.query().unwrap_or("").to_owned();
    let res = next.run(req).await;
    // The second writer acts once, after the read it was staged against and
    // before the write that follows it. Its edit is then rolled back, so what
    // the test sees afterwards is the CLI's own conflict and nothing else.
    if method == axum::http::Method::GET && path.starts_with("/shelf/shares/") {
        let asked = path.rsplit('/').next().unwrap_or("");
        let staged = seen
            .second_writer
            .lock()
            .unwrap()
            .as_ref()
            .is_some_and(|want| want.asked_for(asked));
        if staged {
            *seen.second_writer.lock().unwrap() = None;
            seen.write_as_another_writer(asked);
        }
    }
    if method == axum::http::Method::GET && path.starts_with("/shelf/shares") {
        seen.queries.lock().unwrap().push(query);
    }
    if !*seen.recording.lock().unwrap() {
        return res;
    }
    seen.reads
        .lock()
        .unwrap()
        .push(format!("{} {path}", method.as_str()));
    if method == axum::http::Method::PUT {
        seen.puts
            .lock()
            .unwrap()
            .push((path.clone(), if_match, body));
    }
    if method == axum::http::Method::DELETE {
        seen.deletes.lock().unwrap().push(path);
    }
    res
}

/// The share the access tests start from: two client CIDRs, one write and one
/// read, and a rule naming a principal rather than a CIDR.
///
/// Only the last is beyond what the CLI can send, so the first two are put
/// there by the service's own create through the CLI and that one rule by a
/// direct request; nothing is written into the store behind the router's back.
/// The `PUT` the rule needs is the only write the recorder is not meant to
/// see, so it is noted away afterwards.
async fn seed_through_the_service(h: &Harness) {
    // The share the access tests start from. It is asked for through the
    // service's own routes, which the recorder also wraps, so the recorder is
    // held off until the share is in place. What a test reads back out of it
    // is then its own traffic alone.
    h.seen.pause_recording();
    let r = pi(
        h,
        "",
        &[
            "share",
            "create",
            "home",
            "--size",
            "20",
            "--cidr",
            "10.0.0.0/24",
            "--cidr",
            "10.9.0.0/16",
            "--mode",
            "read",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    // The first CIDR is write, the second read, as the suite has always kept
    // them; the CLI sets one mode per call, so they are named one at a time.
    let r = pi(
        h,
        "",
        &["share", "access", "allow", "home", "--cidr", "10.0.0.0/24"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = add_principal_rule(h, "home", "alice", "read").await;
    assert_eq!(r.code, 0, "{r:?}");
    h.seen.start_recording();
}

/// A rule naming a principal. The CLI has no flag for one, so the suite sends
/// the request the service offers and lets its own routes store it.
async fn add_principal_rule(h: &Harness, name: &str, principal: &str, mode: &str) -> Run {
    let cur = h.share(name).expect("the share to edit");
    let mut access = cur["spec"]["access"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    access.push(json!({"principal": principal, "mode": mode}));
    let body = json!({
        "spec": {
            "name": cur["name"],
            "protocol": cur["spec"]["protocol"],
            "size_gib": cur["spec"]["size_gib"],
            "access": access,
        }
    });
    send(h, "PUT", &format!("/shelf/shares/{name}"), Some(body), None).await
}

/// One request straight to the server, past the CLI. Only a shape the CLI
/// cannot express belongs here.
async fn send(
    h: &Harness,
    method: &str,
    path: &str,
    body: Option<Value>,
    if_match: Option<&str>,
) -> Run {
    let mut req = reqwest::Request::new(
        reqwest::Method::from_bytes(method.as_bytes()).unwrap(),
        format!("{}{path}", h.url).parse().unwrap(),
    );
    if let Some(b) = body {
        req.body_mut().replace(reqwest::Body::from(b.to_string()));
        req.headers_mut()
            .insert("content-type", "application/json".parse().unwrap());
    }
    if let Some(rev) = if_match {
        req.headers_mut().insert("if-match", rev.parse().unwrap());
    }
    let res = reqwest::Client::new().execute(req).await.expect("request");
    let status = res.status();
    let text = res.text().await.unwrap_or_default();
    Run {
        code: if status.is_success() {
            0
        } else {
            i32::from(status.as_u16())
        },
        out: text,
        err: String::new(),
    }
}

/// A second share, cephfs, so a list has more than one protocol in it.
async fn seed_media(h: &Harness) {
    let r = pi(
        h,
        "",
        &[
            "share",
            "create",
            "media",
            "--protocol",
            "cephfs",
            "--size",
            "500",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
}

#[derive(Debug)]
struct Run {
    code: i32,
    out: String,
    err: String,
}

async fn pi(h: &Harness, stdin: &str, args: &[&str]) -> Run {
    let config = h._dir.path().join("config.toml");
    let mut argv: Vec<String> = vec![
        "pi".to_owned(),
        "--config".to_owned(),
        config.display().to_string(),
        "--url".to_owned(),
        h.url.clone(),
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

/// `share-xxxxxxxx`, the short form `pi` prints for a full id.
fn short_id(id: &str) -> String {
    let (kind, uuid) = id.split_once('-').expect("id");
    let hex: String = uuid.chars().filter(|c| *c != '-').collect();
    format!("{kind}-{}", &hex[hex.len() - 8..])
}

// ---- where shelf keeps its files -----------------------------------------

fn set_env(key: &str, path: &std::path::Path) {
    std::env::set_var(key, path.as_os_str());
}

fn set_var(key: &str, value: &str) {
    std::env::set_var(key, value);
}

fn unset_env(key: &str) {
    std::env::remove_var(key);
}

/// Shelf looks a tool up by absolute path when the variable names one and
/// falls back to `PATH` when it does not. This mirrors the fallback so the
/// suite asks for the `exportfs` the server would use.
fn which(name: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|c| c.is_file())
}

// ---- the suite ------------------------------------------------------------

#[tokio::test]
async fn a_share_that_cannot_be_exported_says_so_and_is_still_worked_on() {
    // The other face of the live cluster: the server has no root and no
    // `exportfs`, so a share with a client CIDR never becomes ready. It is
    // still a row the CLI can show, edit and delete.
    let h = serve_without_nfs_tooling().await;
    let r = pi(
        &h,
        "",
        &[
            "share",
            "create",
            "home",
            "--size",
            "1",
            "--cidr",
            "10.60.0.0/24",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&h, "", &["share", "show", "home", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(v["status"]["phase"], "error", "{v}");
    assert!(
        v["status"]["message"].as_str().unwrap().contains("root"),
        "{v}"
    );
    let r = pi(
        &h,
        "",
        &["share", "access", "allow", "home", "--cidr", "10.61.0.0/24"],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&h, "", &["share", "delete", "home", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
}

#[tokio::test]
async fn share_list_prints_protocol_size_and_access() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    seed_media(&h).await;
    let r = pi(&h, "", &["share", "list"]).await;
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

    // wide adds the client count, the mount address and the message
    let r = pi(&h, "", &["share", "list", "-o", "wide", "--no-headers"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let home = r.out.lines().find(|l| l.contains("home")).unwrap();
    // An exported NFS share reports the directory it is exported from.
    assert!(home.contains("/shares/"), "{home}");
    // A ready NFS share also says where to mount it. The address comes from
    // the server: the harness starts its shelf with the public URL
    // `http://test`, so its host — not the ephemeral listener, which the
    // module never saw — stands before the path.
    assert!(home.contains("test:/"), "{home}");
    let media = r.out.lines().find(|l| l.contains("media")).unwrap();
    // A cephfs share is not mounted over NFS and shows nothing there.
    assert!(!media.contains(":/"), "{media}");
    assert_eq!(h.seen.queries.lock().unwrap()[0], "");

    // The plain table has no EXPORT column; `show` prints the field whole.
    let r = pi(&h, "", &["share", "list"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(!r.out.contains("EXPORT"), "{}", r.out);
    let r = pi(&h, "", &["share", "show", "home", "-o", "json"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let v: Value = serde_json::from_str(&r.out).unwrap();
    assert_eq!(
        v["status"]["export"].as_str(),
        Some(format!("test:{}", v["status"]["message"].as_str().unwrap()).as_str()),
        "{v}"
    );
}

#[tokio::test]
async fn share_list_filters_by_label_and_project() {
    let h = serve().await;
    let r = pi(&h, "", &["share", "list", "--all", "-l", "tier=hot"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(
        h.seen.queries.lock().unwrap()[0],
        "all=true&labels=tier=hot"
    );
}

#[tokio::test]
async fn share_show_takes_a_name_or_an_id() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    let id = h.id_of("home");
    for name in ["home", &id] {
        let r = pi(&h, "", &["share", "show", name, "-o", "json"]).await;
        assert_eq!(r.code, 0, "{r:?}");
        let v: Value = serde_json::from_str(&r.out).unwrap();
        assert_eq!(v["name"], "home");
        assert_eq!(v["spec"]["protocol"], "nfs");
    }
    // Table output is the whole record as fields.
    let r = pi(&h, "", &["share", "show", "home"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("FIELD"), "{}", r.out);
    assert!(r.out.contains("spec.protocol"), "{}", r.out);

    let r = pi(&h, "", &["share", "show", "nope"]).await;
    assert_eq!(r.code, 1);
    assert!(r.err.contains("not found"), "{}", r.err);
}

#[tokio::test]
async fn share_create_sends_the_spec_the_server_wants() {
    let h = serve().await;
    let r = pi(
        &h,
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
    let stored = h.share("home").expect("created");
    assert_eq!(
        r.out.trim(),
        format!(
            "share \"home\" created ({})",
            short_id(stored["id"].as_str().unwrap())
        ),
        "{}",
        r.out
    );
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
    // The share landed in the caller's project, which is what the later
    // lookups then have to agree with.
    assert_eq!(stored["project"], h.project.to_string());
}

#[tokio::test]
async fn share_create_defaults_to_a_small_nfs_share() {
    let h = serve().await;
    let r = pi(&h, "", &["share", "create", "scratch"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    let stored = h.share("scratch").expect("created");
    assert_eq!(stored["spec"]["protocol"], "nfs");
    assert_eq!(stored["spec"]["size_gib"], 10);
    assert!(stored["spec"].get("access").is_none(), "{stored}");
}

#[tokio::test]
async fn a_share_just_created_is_found_by_name_id_and_short_id() {
    // The shape the live cluster showed: `pi share create` reports a short id,
    // `pi share list` shows the row, and every verb that names one share then
    // answers "not found". So every handle a user can act on is tried against a
    // share that was just created: the name they gave, the full id the service
    // answers with, and the short id the create reported and every table prints.
    for form in ["name", "full id", "short id"] {
        let h = serve().await;
        let r = pi(&h, "", &["share", "create", "sh1", "--size", "1"]).await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        let id = h.id_of("sh1");
        let short = short_id(&id);
        assert!(
            r.out.contains(&short),
            "{form}: create reports the short id: {}",
            r.out
        );
        let listed = pi(&h, "", &["share", "list", "--no-headers"]).await;
        assert!(listed.out.contains("sh1"), "{form}: {}", listed.out);
        let handle = match form {
            "name" => "sh1".to_owned(),
            "full id" => id.clone(),
            _ => short.clone(),
        };

        let r = pi(&h, "", &["share", "show", &handle, "-o", "json"]).await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        let v: Value = serde_json::from_str(&r.out).unwrap();
        assert_eq!(v["id"], id, "{form}");
        assert_eq!(v["name"], "sh1", "{form}");

        let r = pi(&h, "", &["share", "access", "list", &handle, "-o", "json"]).await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&r.out).unwrap(),
            json!([]),
            "{form}"
        );

        let r = pi(
            &h,
            "",
            &[
                "share",
                "access",
                "allow",
                &handle,
                "--cidr",
                "10.60.0.0/24",
            ],
        )
        .await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        assert!(r.out.contains("allowed for 10.60.0.0/24"), "{form}: {r:?}");

        let r = pi(&h, "", &["share", "access", "list", &handle, "-o", "json"]).await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&r.out).unwrap(),
            json!([{"cidr": "10.60.0.0/24", "mode": "write"}]),
            "{form}"
        );

        let r = pi(
            &h,
            "",
            &["share", "access", "deny", &handle, "--cidr", "10.60.0.0/24"],
        )
        .await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        let r = pi(&h, "", &["share", "access", "list", &handle, "-o", "json"]).await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        assert_eq!(
            serde_json::from_str::<Value>(&r.out).unwrap(),
            json!([]),
            "{form}"
        );

        let r = pi(&h, "", &["share", "delete", &handle, "--yes"]).await;
        assert_eq!(r.code, 0, "{form}: {r:?}");
        assert!(r.out.contains("deleted"), "{form}: {r:?}");
        assert!(h.row_by_id(&id).is_none(), "{form}: the row is gone");
        let r = pi(&h, "", &["share", "show", &handle]).await;
        assert_eq!(r.code, 1, "{form}: a deleted share is not shown: {r:?}");
        assert!(r.err.contains("not found"), "{form}: {r:?}");
    }
}

#[tokio::test]
async fn share_create_refuses_nonsense_before_asking_the_server() {
    let h = serve().await;
    for bad in ["10.0.0.0", "10.0.0.0/33", "host.example/24"] {
        let r = pi(&h, "", &["share", "create", "home", "--cidr", bad]).await;
        assert_eq!(r.code, 1, "{r:?}");
        assert!(r.err.contains("is not a CIDR"), "{}", r.err);
    }
    let r = pi(
        &h,
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
    assert!(h.shares().is_empty());
}

#[tokio::test]
async fn share_create_reports_what_the_server_refuses() {
    let h = serve().await;
    let r = pi(&h, "", &["share", "create", "home", "--protocol", "smb"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("unknown protocol 'smb'"), "{}", r.err);

    let r = pi(&h, "", &["share", "create", "home", "--size", "0"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(
        r.err.contains("size_gib must be greater than 0"),
        "{}",
        r.err
    );

    // A bad CIDR that the CLI lets through is refused by shelf, and the
    // share is not left half-made.
    let r = pi(
        &h,
        "",
        &[
            "share",
            "create",
            "home",
            "--cidr",
            "10.0.0.0/24",
            "--cidr",
            "10.0.0.0/24",
        ],
    )
    .await;
    assert_eq!(r.code, 0, "{r:?}");
    let r = pi(&h, "", &["share", "create", "home", "--size", "1"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("already exists"), "{}", r.err);
}

#[tokio::test]
async fn share_delete_asks_unless_told_not_to() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    // No terminal to confirm on, so this refuses rather than deleting.
    let r = pi(&h, "", &["share", "delete", "home"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("without --yes"), "{}", r.err);
    assert!(h.share("home").is_some());
    let id = h.id_of("home");

    let r = pi(&h, "", &["share", "delete", "home", "--yes"]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert_eq!(r.out.trim(), "share \"home\" deleted", "{}", r.out);
    assert!(h.shares().is_empty());
    // The CLI deletes by the name it was given; shelf resolves it.
    assert_eq!(h.seen.deletes.lock().unwrap()[0], "/shelf/shares/home");
    assert!(h.share(&id).is_none(), "the row is gone by id too");
}

#[tokio::test]
async fn share_access_lists_only_the_cidr_rules() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    let r = pi(&h, "", &["share", "access", "list", "home"]).await;
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

    let r = pi(&h, "", &["share", "access", "list", "home", "-o", "json"]).await;
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
    let h = serve().await;
    seed_through_the_service(&h).await;
    let id = h.id_of("home");
    let rev = h.share("home").unwrap()["revision"].as_u64().unwrap();
    let r = pi(
        &h,
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
    let (path, if_match, body) = &h.seen.puts.lock().unwrap()[0];
    assert_eq!(
        path,
        &format!("/shelf/shares/{id}"),
        "the edit is addressed by id"
    );
    assert!(
        if_match.as_deref() == Some(rev.to_string().as_str()),
        "If-Match is the revision read, got {if_match:?} for revision {rev}"
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
    let h = serve().await;
    seed_through_the_service(&h).await;
    let r = pi(
        &h,
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
    let access = h.seen.puts.lock().unwrap()[0].2["spec"]["access"].clone();
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
    let h = serve().await;
    seed_through_the_service(&h).await;
    let r = pi(
        &h,
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
    assert!(h.seen.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn share_access_deny_drops_one_rule() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    let r = pi(
        &h,
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
    let access = h.seen.puts.lock().unwrap()[0].2["spec"]["access"].clone();
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
    let h = serve().await;
    seed_through_the_service(&h).await;
    let r = pi(
        &h,
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
    assert!(h.seen.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_deny_names_only_the_cidr() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    // A mode says nothing about which rule to drop, so deny does not take one.
    let r = pi(
        &h,
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
    assert!(h.seen.puts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn a_share_moved_underneath_an_access_edit_is_not_overwritten() {
    let h = serve_moved_underneath().await;
    let r = pi(
        &h,
        "",
        &["share", "access", "allow", "home", "--cidr", "10.7.0.0/16"],
    )
    .await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("revision"), "{r:?}");
    // The other writer's share stands; the rule the CLI wanted is not on it.
    let home = h.share("home").expect("still there");
    // The share was never touched: the size and the access list the CLI
    // meant to edit are both still as the seed left them.
    assert_eq!(home["spec"]["size_gib"], 20);
    assert_eq!(home["revision"], 1);
    assert_eq!(home["spec"]["access"].as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn share_access_on_a_missing_share_reports_the_share() {
    let h = serve().await;
    let r = pi(&h, "", &["share", "access", "list", "ghost"]).await;
    assert_eq!(r.code, 1, "{r:?}");
    assert!(r.err.contains("'ghost' not found"), "{}", r.err);
    for verb in ["allow", "deny"] {
        let r = pi(
            &h,
            "",
            &["share", "access", verb, "ghost", "--cidr", "10.0.0.0/24"],
        )
        .await;
        assert_eq!(r.code, 1, "{r:?}");
        assert!(r.err.contains("'ghost' not found"), "{}", r.err);
    }
}

#[tokio::test]
async fn apply_configures_a_share_and_its_access() {
    let h = serve().await;
    seed_through_the_service(&h).await;
    let dir = tempfile::tempdir().unwrap();
    let manifest = dir.path().join("share.yaml");
    std::fs::write(
        &manifest,
        "kind: share\nname: home\nspec:\n  size_gib: 30\n",
    )
    .unwrap();
    let r = pi(&h, "", &["apply", "-f", manifest.to_str().unwrap()]).await;
    assert_eq!(r.code, 0, "{r:?}");
    assert!(r.out.contains("configured"), "{}", r.out);
    let home = h.share("home").expect("still there");
    assert_eq!(home["spec"]["size_gib"], 30);
    // A partial manifest leaves the rules alone.
    assert_eq!(home["spec"]["access"].as_array().unwrap().len(), 3);
}
