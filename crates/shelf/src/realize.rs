// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Turn an NFS share into a directory and one managed exports file.
//! CephFS creates a subvolume when `ceph` is configured.
//!
//! The share root is `$PISTACK_SHELF_DIR`, otherwise
//! `$PISTACK_DATA_DIR/shares`, otherwise `./data/shares`. The server creates
//! that directory, and each share directory under it, itself.
//!
//! # Privilege boundary
//!
//! Shelf owns one file, `/etc/exports.d/pistack.exports`
//! (`PISTACK_EXPORTS_FILE` overrides the path). Every pass rewrites that
//! file from the desired shares. Nothing else under `/etc/exports` or
//! `/etc/exports.d` is read or written. The file is what an nfs server
//! restart reloads. `exportfs -ra` loads it into the live table.
//!
//! Root writes the file and runs `exportfs -ra` in this process.
//! `PISTACK_EXPORTFS` overrides the `exportfs` binary.
//!
//! Every managed line squashes its clients
//! (`all_squash,anonuid=65534,anongid=65534`, see [`crate::export_opts`])
//! and each share directory is owned by that uid with mode `0770` plus
//! setgid (see [`crate::share_dir`]), so a `write` share is writable by any
//! client user and a `read` share is not.
//!
//! Any other user runs both steps through one helper named by
//! `PISTACK_SHELF_HELPER`. The program in this repo is
//! `scripts/pistack-shelf-helper`. Install it as
//! `/usr/local/sbin/pistack-shelf-helper` and point the variable at that
//! path. The call matches fabric's host helper: one program, `sudo -n`,
//! arguments, and a stdin payload. No shelf daemon and no second API.
//!
//! ```text
//! sudo -n "$PISTACK_SHELF_HELPER" apply --file <exports-path>
//! ```
//!
//! Stdin is the full file body. The helper creates the parent of
//! `<exports-path>` when it is missing (normally `/etc/exports.d`), replaces
//! that one file with exactly those bytes, mode `0644`, and runs
//! `exportfs -ra`. It exits 0 only when the write and `exportfs -ra` both
//! succeed. It refuses any path that is not one file directly under
//! `/etc/exports.d` ending in `.exports`. A sudoers rule for the server user
//! names only that command, with `NOPASSWD`:
//!
//! ```text
//! pistack ALL=(root) NOPASSWD: /usr/local/sbin/pistack-shelf-helper
//! ```
//!
//! `PISTACK_SHELF_EUID`, when set, overrides the process euid so tests can
//! take either path. Leave it unset on a server.
//!
//! A failed export is [`Phase::Error`] with the helper or `exportfs` text in
//! `status.message`. The reconcile loop retries it with exponential backoff.
//! A ready NFS share's `status.export` carries `<address>:<path>`, the
//! address being this server's API host (see [`crate::export_address`]).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::Duration;

use chrono::{DateTime, Utc};
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id, Phase};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use crate::export_address;
use crate::export_opts::export_line;
use crate::share_dir;
use crate::types::{AccessMode, Share, ShareStatus, PROTOCOL_CEPHFS, PROTOCOL_NFS};

const DEFAULT_EXPORTS: &str = "/etc/exports.d/pistack.exports";
const MANAGED_HEADER: &str = "\
# Managed by pistack shelf. Rewritten from desired share state.\n\
# Do not edit. Removing a share drops its lines on the next pass.\n\
# exportfs -ra, or an nfs server restart, reloads this file.\n";

pub const NO_CEPH: &str = "ceph is not installed";
pub const NO_CEPH_FS: &str = "ceph fs name is not configured";
pub const NO_CIDR: &str = "nfs share has no client cidr";
const NEED_HELPER: &str = "nfs exports need root or PISTACK_SHELF_HELPER";

/// How often the reconcile loop looks for a share whose backoff has elapsed.
pub fn retry_interval() -> Duration {
    Duration::from_millis(env_u64("PISTACK_SHELF_RETRY_MS", 5_000))
}

fn shelf_root() -> PathBuf {
    if let Some(v) = std::env::var_os("PISTACK_SHELF_DIR") {
        if !v.is_empty() {
            return PathBuf::from(v);
        }
    }
    let base = match std::env::var_os("PISTACK_DATA_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from("./data"),
    };
    base.join("shares")
}

fn exports_file() -> PathBuf {
    match std::env::var_os("PISTACK_EXPORTS_FILE") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_EXPORTS),
    }
}

fn export_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
}

/// Block until the in-flight export pass releases the lock.
pub async fn quiesce() {
    let _guard = export_lock().lock().await;
}

/// Create the share root. The error names the path.
pub async fn ensure_share_root() -> ApiResult<PathBuf> {
    create_root().await.map_err(ApiError::internal)
}

/// Realize `share` together with the shares already stored. `share` itself
/// is not written; the caller stores it. Other shares are updated when
/// their status changes.
///
/// A share not stored yet is worked on from the revision it will be filed at,
/// which is the one [`pistack_store::Collection::create`] sets. Left at the zero
/// its record is built with, the status handed back says "nothing of this has
/// been observed", and the answer to a create is a share forever waiting on
/// itself: `pi share show` still says `pending` a minute later, because the
/// reconciler's own "is there work here" test is a status that has not caught up
/// with the record — and a pass that fails backs off before it looks again.
pub async fn prepare(store: &Store, share: &mut Share) -> ApiResult<()> {
    reject_bad_cidr(share)?;
    let _guard = export_lock().lock().await;
    let mut all = list_shares(store)?;
    let id = share.meta.id.clone();
    if let Some(pos) = all.iter().position(|s| s.meta.id == id) {
        all[pos] = share.clone();
    } else {
        share.status.observed_revision = share.meta.revision;
        all.push(share.clone());
    }
    let _publish = drive(&mut all, Utc::now(), Some(&id), false).await;
    if let Some(done) = all.iter().find(|s| s.meta.id == id) {
        share.status = done.status.clone();
    }
    persist(store, &all, Some(&id))
}

/// Drop `share` from the managed exports file, reload `exportfs`, and
/// remove its directory. The caller deletes the store record after `Ok`.
pub async fn unpublish(store: &Store, share: &Share) -> ApiResult<()> {
    let _guard = export_lock().lock().await;
    let id = share.meta.id.to_string();
    safe_id(&id)?;
    let mut all = list_shares(store)?;
    all.retain(|s| s.meta.id != share.meta.id);
    let reload = exports_file().exists()
        || (share.spec.protocol == PROTOCOL_NFS
            && has_cidr(share)
            && share.status.phase == Phase::Ready);
    if reload {
        if let Some(err) = drive(&mut all, Utc::now(), None, true).await {
            let _ = persist(store, &all, None);
            return Err(ApiError::unavailable(err));
        }
        persist(store, &all, None)?;
    }
    remove_on_disk(share).await?;
    if share.spec.protocol == PROTOCOL_CEPHFS {
        remove_ceph(share, &id).await?;
    }
    Ok(())
}

/// Retry shares whose backoff has elapsed, and rewrite the exports file
/// when it no longer matches shares that are already `Ready`.
pub async fn reconcile_due(store: &Store) -> ApiResult<()> {
    let _guard = export_lock().lock().await;
    let mut shares = list_shares(store)?;
    let now = Utc::now();
    let due = shares.iter().any(|s| share_due(s, now));
    let drift = !due && exports_drifted(&shares);
    if !due && !drift {
        return Ok(());
    }
    let _publish = drive(&mut shares, now, None, drift).await;
    persist(store, &shares, None)
}

/// Realize one share and publish a file that contains only that share.
/// Unit tests call this. The server uses [`prepare`].
#[cfg(test)]
pub async fn apply(share: &mut Share) -> ApiResult<()> {
    reject_bad_cidr(share)?;
    let _guard = export_lock().lock().await;
    let mut all = vec![share.clone()];
    let _publish = drive(&mut all, Utc::now(), Some(&share.meta.id), false).await;
    share.status = all.remove(0).status;
    Ok(())
}

/// Remove one share's directory and its lines in the managed file.
/// Unit tests call this. The server uses [`unpublish`].
#[cfg(test)]
pub async fn remove(share: &Share) -> ApiResult<()> {
    let _guard = export_lock().lock().await;
    let id = share.meta.id.to_string();
    safe_id(&id)?;
    if share.spec.protocol == PROTOCOL_NFS && exports_file().exists() {
        let old = read_exports().await;
        let body = strip_share_lines(&old, &id);
        if body != old {
            publish_body(&body).await.map_err(ApiError::unavailable)?;
        }
    }
    remove_on_disk(share).await?;
    if share.spec.protocol == PROTOCOL_CEPHFS {
        remove_ceph(share, &id).await?;
    }
    Ok(())
}

fn list_shares(store: &Store) -> ApiResult<Vec<Share>> {
    store
        .collection::<Share>()
        .list(None)
        .map_err(ApiError::from)
}

fn persist(store: &Store, shares: &[Share], skip: Option<&Id>) -> ApiResult<()> {
    let col = store.collection::<Share>();
    for share in shares {
        if skip.is_some_and(|id| id == &share.meta.id) {
            continue;
        }
        let current = match col.try_get(&share.meta.id).map_err(ApiError::from)? {
            Some(v) => v,
            None => continue,
        };
        if !status_changed(&current.status, &share.status)
            && current.status.observed_revision == current.meta.revision
        {
            continue;
        }
        col.modify(&share.meta.id, |cur| {
            cur.status = share.status.clone();
            cur.status.observed_revision = cur.meta.revision + 1;
            Ok(())
        })
        .map_err(ApiError::from)?;
    }
    Ok(())
}

fn status_changed(current: &ShareStatus, next: &ShareStatus) -> bool {
    current.phase != next.phase
        || current.message != next.message
        || current.export != next.export
        || current.failures != next.failures
        || current.retry_at != next.retry_at
}

struct Plan {
    index: usize,
    id: String,
    dir: String,
    lines: String,
    due_now: bool,
}

struct Pass<'a> {
    now: DateTime<Utc>,
    force: Option<&'a Id>,
    force_publish: bool,
}

/// Returns the export error when a reload was attempted and failed.
/// Share statuses already describe that failure.
async fn drive(
    shares: &mut [Share],
    now: DateTime<Utc>,
    force: Option<&Id>,
    force_publish: bool,
) -> Option<String> {
    let pass = Pass {
        now,
        force,
        force_publish,
    };
    let mut plans = Vec::new();
    let root = if shares.iter().any(nfs_with_cidr) {
        match create_root().await {
            Ok(path) => Some(path),
            Err(msg) => {
                for share in shares.iter_mut() {
                    if nfs_with_cidr(share) && should_attempt(share, &pass) {
                        mark_failure(share, msg.clone());
                    }
                }
                None
            }
        }
    } else {
        None
    };

    for (index, share) in shares.iter_mut().enumerate() {
        let protocol = share.spec.protocol.clone();
        let id = share.meta.id.to_string();
        if protocol == PROTOCOL_CEPHFS {
            if pass.force == Some(&share.meta.id) || share_due(share, pass.now) {
                if let Err(e) = safe_id(&id) {
                    mark_failure(share, e.message);
                } else if let Err(message) = realize_ceph(share, &id).await {
                    // realize_ceph records Pending itself and returns Ok.
                    // A hard error is still shown.
                    mark_failure(share, message);
                }
            }
            continue;
        }
        if protocol != PROTOCOL_NFS {
            mark_failure(share, format!("unknown protocol '{protocol}'"));
            continue;
        }
        if let Err(e) = safe_id(&id) {
            mark_failure(share, e.message);
            continue;
        }
        if !has_cidr(share) {
            if pass.force == Some(&share.meta.id)
                || share.status.phase != Phase::Pending
                || share.status.message != NO_CIDR
            {
                mark_pending(share, NO_CIDR);
            }
            continue;
        }
        if let Some(msg) = bad_cidr_message(share) {
            if should_attempt(share, &pass) {
                mark_failure(share, msg);
            }
            continue;
        }
        let Some(root) = root.as_ref() else {
            continue;
        };
        let due_now = pass.force == Some(&share.meta.id) || share_due(share, pass.now);
        if should_attempt(share, &pass) {
            match create_share_dir(root, &id).await {
                Ok(dir) => plans.push(plan_at(index, share, dir, due_now)),
                Err(msg) => mark_failure(share, msg),
            }
        } else if let Some(dir) = existing_share_dir(root, &id) {
            plans.push(plan_at(index, share, dir, false));
        }
    }

    if plans.is_empty() && !exports_file().exists() {
        return None;
    }
    let old = read_exports().await;
    let body = render(&plans);
    let exists = exports_file().exists();
    let changed = (exists && body != old) || (!exists && !plans.is_empty());
    let due = plans.iter().any(|p| p.due_now);
    if !pass.force_publish && !changed && !due {
        return None;
    }
    match publish_body(&body).await {
        Ok(()) => {
            for plan in &plans {
                mark_ready(&mut shares[plan.index], plan.dir.clone());
            }
            None
        }
        Err(msg) => {
            tracing::warn!(error = %msg, "shelf: nfs export failed");
            for plan in &plans {
                let same = section(&old, &plan.dir) == plan.lines;
                if same && !plan.due_now {
                    continue;
                }
                mark_failure(&mut shares[plan.index], msg.clone());
            }
            Some(msg)
        }
    }
}

fn plan_at(index: usize, share: &Share, dir: String, due_now: bool) -> Plan {
    let lines = cidr_rules(share)
        .into_iter()
        .map(|(cidr, mode)| export_line(&dir, &cidr, mode))
        .collect();
    Plan {
        index,
        id: share.meta.id.to_string(),
        dir,
        lines,
        due_now,
    }
}

fn should_attempt(share: &Share, pass: &Pass<'_>) -> bool {
    pass.force == Some(&share.meta.id)
        || share.status.phase != Phase::Error
        || share_due(share, pass.now)
}

fn share_due(share: &Share, now: DateTime<Utc>) -> bool {
    if share.status.observed_revision != share.meta.revision {
        return true;
    }
    share.status.phase == Phase::Error && share.status.retry_at.is_none_or(|t| now >= t)
}

fn exports_drifted(shares: &[Share]) -> bool {
    let Some(desired) = ready_document(shares) else {
        return false;
    };
    match std::fs::read_to_string(exports_file()) {
        Ok(body) => body != desired,
        Err(_) => document_has_exports(&desired),
    }
}

/// `None` when some NFS share with a client CIDR is not `Ready`: that share
/// owns the next pass through its backoff. Otherwise the document every
/// `Ready` share should produce.
fn ready_document(shares: &[Share]) -> Option<String> {
    let mut plans = Vec::new();
    for share in shares {
        if !nfs_with_cidr(share) {
            continue;
        }
        if share.status.phase != Phase::Ready {
            return None;
        }
        let dir = share.status.message.clone();
        let lines = cidr_rules(share)
            .into_iter()
            .map(|(cidr, mode)| export_line(&dir, &cidr, mode))
            .collect();
        plans.push(Plan {
            index: 0,
            id: share.meta.id.to_string(),
            dir,
            lines,
            due_now: false,
        });
    }
    Some(render(&plans))
}

fn document_has_exports(body: &str) -> bool {
    body.lines()
        .any(|line| !line.is_empty() && !line.starts_with('#'))
}

fn render(plans: &[Plan]) -> String {
    let mut ordered: Vec<&Plan> = plans.iter().collect();
    ordered.sort_by(|a, b| a.id.cmp(&b.id));
    let mut body = String::from(MANAGED_HEADER);
    for plan in ordered {
        body.push_str(&plan.lines);
    }
    body
}

fn section(body: &str, dir: &str) -> String {
    let prefix = format!("{dir} ");
    let mut out = String::new();
    for line in body.lines() {
        if line.starts_with(&prefix) {
            out.push_str(line);
            out.push('\n');
        }
    }
    out
}

async fn publish_body(body: &str) -> Result<(), String> {
    if running_as_root() {
        let bin = tool_bin("PISTACK_EXPORTFS", "exportfs")
            .ok_or_else(|| "exportfs is not installed".to_owned())?;
        write_exports_file(body).await?;
        return run_tool(&bin, &["-ra"]).await;
    }
    let helper = helper_program()?;
    run_helper(&helper, body).await
}

async fn write_exports_file(body: &str) -> Result<(), String> {
    let path = exports_file();
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .ok_or_else(|| {
            format!(
                "cannot create exports file {}: no parent directory",
                path.display()
            )
        })?;
    tokio::fs::create_dir_all(parent)
        .await
        .map_err(|e| format!("cannot create {}: {e}", parent.display()))?;
    let tmp = parent.join(format!(".pistack-exports-{}.tmp", std::process::id()));
    tokio::fs::write(&tmp, body)
        .await
        .map_err(|e| format!("cannot write {}: {e}", tmp.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        tokio::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))
            .await
            .map_err(|e| format!("cannot set mode on {}: {e}", tmp.display()))?;
    }
    if let Err(e) = tokio::fs::rename(&tmp, &path).await {
        let _ = tokio::fs::remove_file(&tmp).await;
        return Err(format!("cannot replace {}: {e}", path.display()));
    }
    Ok(())
}

async fn read_exports() -> String {
    tokio::fs::read_to_string(exports_file())
        .await
        .unwrap_or_default()
}

fn helper_program() -> Result<PathBuf, String> {
    let raw = std::env::var("PISTACK_SHELF_HELPER").unwrap_or_default();
    let raw = raw.trim();
    if raw.is_empty() || raw.contains('\0') || raw.contains('\n') || raw.contains('\r') {
        return Err(NEED_HELPER.to_owned());
    }
    Ok(PathBuf::from(raw))
}

async fn run_helper(program: &Path, body: &str) -> Result<(), String> {
    let path = exports_file();
    let args = vec![
        "apply".to_owned(),
        "--file".to_owned(),
        path.display().to_string(),
    ];
    run_program(
        program,
        true,
        &args,
        Some(body.as_bytes()),
        "nfs export helper",
    )
    .await
}

/// `sudo -n <program> <args>` when `sudo` is set, otherwise `<program>`.
/// Same shape as fabric's host helper.
async fn run_program(
    program: &Path,
    sudo: bool,
    args: &[String],
    stdin_body: Option<&[u8]>,
    what: &str,
) -> Result<(), String> {
    let mut cmd = if sudo {
        let mut c = Command::new("sudo");
        c.arg("-n").arg(program);
        c
    } else {
        Command::new(program)
    };
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(if stdin_body.is_some() {
            std::process::Stdio::piped()
        } else {
            std::process::Stdio::null()
        })
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    let mut child = crate::spawn_retry::spawn_retrying_busy(|| cmd.spawn())
        .await
        .map_err(|e| format!("{what} {}: {e}", program.display()))?;
    if let Some(body) = stdin_body {
        if let Some(mut input) = child.stdin.take() {
            if let Err(e) = input.write_all(body).await {
                let _ = child.kill().await;
                return Err(format!("{what} {}: {e}", program.display()));
            }
        }
    }
    match tokio::time::timeout(Duration::from_secs(30), child.wait_with_output()).await {
        Ok(Ok(out)) if out.status.success() => Ok(()),
        Ok(Ok(out)) => {
            let err = String::from_utf8_lossy(&out.stderr);
            let extra = String::from_utf8_lossy(&out.stdout);
            let detail = clip(&format!("{err} {extra}"));
            if detail.is_empty() {
                Err(format!("{what} failed"))
            } else {
                Err(format!("{what} failed: {detail}"))
            }
        }
        Ok(Err(e)) => Err(format!("{what} {}: {e}", program.display())),
        Err(_) => Err(format!("{what} timed out")),
    }
}

fn running_as_root() -> bool {
    if let Ok(raw) = std::env::var("PISTACK_SHELF_EUID") {
        let raw = raw.trim();
        if !raw.is_empty() {
            return raw == "0";
        }
    }
    process_euid() == 0
}

fn process_euid() -> u32 {
    // SAFETY: geteuid has no preconditions and does not touch Rust memory.
    unsafe { geteuid() }.try_into().unwrap_or(u32::MAX)
}

extern "C" {
    fn geteuid() -> std::ffi::c_int;
}

fn backoff_ms(failures: u32) -> i64 {
    let base =
        i64::try_from(env_u64("PISTACK_SHELF_BACKOFF_MS", 5_000).min(300_000)).unwrap_or(300_000);
    // Shift 63 would make the factor negative. Anything past ~19 already hits the cap.
    let shift = failures.saturating_sub(1).min(62);
    let factor = 1_i64.checked_shl(shift).unwrap_or(i64::MAX);
    base.saturating_mul(factor).min(300_000)
}

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse::<u64>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(default)
}

fn mark_failure(share: &mut Share, message: String) {
    let failures = if share.status.phase == Phase::Error {
        share.status.failures.saturating_add(1)
    } else {
        1
    };
    let observed = share.status.observed_revision;
    share.status = ShareStatus {
        phase: Phase::Error,
        message,
        export: None,
        failures,
        retry_at: Some(Utc::now() + chrono::Duration::milliseconds(backoff_ms(failures))),
        observed_revision: observed,
    };
}

fn mark_ready(share: &mut Share, dir: String) {
    let observed = share.status.observed_revision;
    let export = export_address::get().map(|addr| format!("{addr}:{dir}"));
    share.status = ShareStatus {
        phase: Phase::Ready,
        message: dir,
        export,
        failures: 0,
        retry_at: None,
        observed_revision: observed,
    };
}

fn mark_pending(share: &mut Share, message: &str) {
    let observed = share.status.observed_revision;
    share.status = ShareStatus {
        phase: Phase::Pending,
        message: message.to_owned(),
        export: None,
        failures: 0,
        retry_at: None,
        observed_revision: observed,
    };
}

async fn create_root() -> Result<PathBuf, String> {
    let root = shelf_root();
    tokio::fs::create_dir_all(&root)
        .await
        .map_err(|e| format!("cannot create share root {}: {e}", root.display()))?;
    tokio::fs::canonicalize(&root)
        .await
        .map_err(|e| format!("cannot open share root {}: {e}", root.display()))
}

async fn create_share_dir(root: &Path, id: &str) -> Result<String, String> {
    safe_id(id).map_err(|e| e.message)?;
    let dir = root.join(id);
    if !dir.starts_with(root) {
        return Err("share path escapes the shelf directory".into());
    }
    if std::fs::symlink_metadata(&dir)
        .ok()
        .is_some_and(|meta| meta.file_type().is_symlink())
    {
        return Err("share path escapes the shelf directory".into());
    }
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| format!("cannot create share directory {}: {e}", dir.display()))?;
    let canon = tokio::fs::canonicalize(&dir)
        .await
        .map_err(|e| format!("cannot open share directory {}: {e}", dir.display()))?;
    if !canon.starts_with(root) || canon == root {
        return Err("share path escapes the shelf directory".into());
    }
    // Every pass, not only the first: a share directory made before the
    // squashed owner existed is fixed here, on the next reconcile. The
    // export line squashes every client to that owner, so a directory it
    // cannot write is the fault this closes. A server that cannot take
    // ownership — no root, and chowning to nobody needs it — still exports:
    // a refusal here would hold back shares that work.
    if let Err(e) = share_dir::apply(&canon).await {
        tracing::warn!(
            dir = %dir.display(),
            error = %e,
            "shelf: share directory keeps the owner it has; nfs clients may not write it",
        );
    }
    Ok(canon.to_string_lossy().into_owned())
}

fn existing_share_dir(root: &Path, id: &str) -> Option<String> {
    let canon = std::fs::canonicalize(root.join(id)).ok()?;
    if canon.starts_with(root) && canon != root {
        Some(canon.to_string_lossy().into_owned())
    } else {
        None
    }
}

async fn remove_on_disk(share: &Share) -> ApiResult<()> {
    let id = share.meta.id.to_string();
    safe_id(&id)?;
    let Some(root) = existing_root().await? else {
        return Ok(());
    };
    remove_dir_inside(&root, &root.join(&id)).await?;
    let legacy = root.join("exports.d").join(format!("{id}.exports"));
    if legacy.starts_with(&root) {
        let _ = remove_file_inside(&root, &legacy).await;
    }
    Ok(())
}

fn nfs_with_cidr(share: &Share) -> bool {
    share.spec.protocol == PROTOCOL_NFS && has_cidr(share)
}

fn has_cidr(share: &Share) -> bool {
    share.spec.access.iter().any(|rule| rule.cidr.is_some())
}

fn cidr_rules(share: &Share) -> Vec<(String, AccessMode)> {
    share
        .spec
        .access
        .iter()
        .filter_map(|rule| rule.cidr.clone().map(|cidr| (cidr, rule.mode)))
        .collect()
}

fn reject_bad_cidr(share: &Share) -> ApiResult<()> {
    if share.spec.protocol != PROTOCOL_NFS {
        return Ok(());
    }
    if let Some(msg) = bad_cidr_message(share) {
        return Err(ApiError::bad_request(msg));
    }
    Ok(())
}

fn bad_cidr_message(share: &Share) -> Option<String> {
    for rule in &share.spec.access {
        if let Some(cidr) = &rule.cidr {
            if !cidr_ok(cidr) {
                return Some(format!("access cidr '{cidr}' is not a CIDR"));
            }
        }
    }
    None
}

async fn realize_ceph(share: &mut Share, id: &str) -> Result<(), String> {
    let Some(bin) = tool_bin("PISTACK_CEPH_BIN", "ceph") else {
        mark_pending(share, NO_CEPH);
        return Ok(());
    };
    let Some(fs) = ceph_fs() else {
        mark_pending(share, NO_CEPH_FS);
        return Ok(());
    };
    let bytes = u64::from(share.spec.size_gib).saturating_mul(1024 * 1024 * 1024);
    let size = bytes.to_string();
    if let Err(message) = run_tool(
        &bin,
        &["fs", "subvolume", "create", &fs, id, "--size", &size],
    )
    .await
    {
        mark_pending(share, &message);
        return Ok(());
    }
    let observed = share.status.observed_revision;
    share.status = ShareStatus {
        phase: Phase::Ready,
        message: format!("{fs}/{id}"),
        export: None,
        failures: 0,
        retry_at: None,
        observed_revision: observed,
    };
    Ok(())
}

async fn remove_ceph(share: &Share, id: &str) -> ApiResult<()> {
    if share.status.phase != Phase::Ready {
        return Ok(());
    }
    let Some(bin) = tool_bin("PISTACK_CEPH_BIN", "ceph") else {
        return Ok(());
    };
    let Some(fs) = ceph_fs() else {
        return Ok(());
    };
    match run_tool(&bin, &["fs", "subvolume", "rm", &fs, id]).await {
        Ok(()) => Ok(()),
        Err(message) if message.contains("No such") || message.contains("not found") => Ok(()),
        Err(message) => Err(ApiError::unavailable(message)),
    }
}

fn ceph_fs() -> Option<String> {
    let raw = std::env::var("PISTACK_CEPH_FS").ok()?;
    let name = raw.trim();
    if name.is_empty()
        || name.len() > 64
        || !name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
    {
        return None;
    }
    Some(name.to_owned())
}

/// `$key` when set (and it must be a file; there is no PATH fallback).
/// Otherwise the named tool on `PATH`.
fn tool_bin(key: &str, name: &str) -> Option<PathBuf> {
    match std::env::var_os(key) {
        Some(v) if !v.is_empty() => {
            let path = PathBuf::from(v);
            path.is_file().then_some(path)
        }
        Some(_) => None,
        None => which(name),
    }
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let cand = dir.join(name);
        if cand.is_file() {
            return Some(cand);
        }
    }
    None
}

async fn run_tool(bin: &Path, args: &[&str]) -> Result<(), String> {
    let owned: Vec<String> = args.iter().copied().map(str::to_owned).collect();
    let name = bin
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("tool")
        .to_owned();
    run_program(bin, false, &owned, None, &name).await
}

fn clip(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    let mut out: String = trimmed.chars().take(180).collect();
    if trimmed.chars().nth(180).is_some() {
        out.push('…');
    }
    out
}

#[cfg(test)]
fn strip_share_lines(body: &str, id: &str) -> String {
    let needle = format!("/{id} ");
    let mut out = String::new();
    for line in body.lines() {
        if line.contains(&needle) {
            continue;
        }
        out.push_str(line);
        out.push('\n');
    }
    if out.is_empty() {
        MANAGED_HEADER.to_owned()
    } else {
        out
    }
}

async fn existing_root() -> ApiResult<Option<PathBuf>> {
    let root = shelf_root();
    match tokio::fs::try_exists(&root).await {
        Ok(false) => Ok(None),
        Ok(true) => Ok(Some(tokio::fs::canonicalize(&root).await.map_err(|e| {
            ApiError::internal(format!("cannot open share root {}: {e}", root.display()))
        })?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(ApiError::internal(format!(
            "cannot open share root {}: {e}",
            root.display()
        ))),
    }
}

fn safe_id(id: &str) -> ApiResult<()> {
    if component_ok(id) && id != "exports.d" && id.len() <= 64 {
        Ok(())
    } else {
        Err(ApiError::bad_request(
            "share id must match [A-Za-z0-9._-] and stay inside the shelf directory",
        ))
    }
}

fn component_ok(name: &str) -> bool {
    !name.is_empty()
        && name != "."
        && name != ".."
        && name.len() <= 80
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

fn cidr_ok(cidr: &str) -> bool {
    if cidr.len() > 64 || cidr.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return false;
    }
    let Some((addr, bits)) = cidr.split_once('/') else {
        return false;
    };
    if addr.parse::<std::net::IpAddr>().is_err() {
        return false;
    }
    match bits.parse::<u8>() {
        Ok(n) if addr.contains(':') => n <= 128,
        Ok(n) => n <= 32,
        Err(_) => false,
    }
}

async fn remove_dir_inside(root: &Path, dir: &Path) -> ApiResult<()> {
    if !dir.starts_with(root) || dir == root {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    let meta = match tokio::fs::symlink_metadata(dir).await {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(ApiError::internal(format!(
                "cannot open share directory {}: {e}",
                dir.display()
            )))
        }
    };
    if meta.file_type().is_symlink() {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    if !meta.is_dir() {
        tokio::fs::remove_file(dir).await.map_err(|e| {
            ApiError::internal(format!(
                "cannot remove share directory {}: {e}",
                dir.display()
            ))
        })?;
        return Ok(());
    }
    let canon = tokio::fs::canonicalize(dir).await.map_err(|e| {
        ApiError::internal(format!(
            "cannot open share directory {}: {e}",
            dir.display()
        ))
    })?;
    if !canon.starts_with(root) || canon == root {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    tokio::fs::remove_dir_all(dir).await.map_err(|e| {
        ApiError::internal(format!(
            "cannot remove share directory {}: {e}",
            dir.display()
        ))
    })?;
    Ok(())
}

async fn remove_file_inside(root: &Path, path: &Path) -> ApiResult<()> {
    if !path.starts_with(root) || path == root {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    match tokio::fs::symlink_metadata(path).await {
        Ok(meta) if meta.file_type().is_symlink() => {
            return Err(ApiError::bad_request(
                "share path escapes the shelf directory",
            ));
        }
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => {
            return Err(ApiError::internal(format!(
                "cannot open {}: {e}",
                path.display()
            )))
        }
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(ApiError::internal(format!(
            "cannot remove {}: {e}",
            path.display()
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env_lock() -> &'static tokio::sync::Mutex<()> {
        static LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| tokio::sync::Mutex::new(()))
    }

    #[test]
    fn export_line_marks_read_and_write() {
        let _guard = share_dir::test_env_lock().lock().unwrap();
        let prev = std::env::var_os("PISTACK_SHELF_SQUASH_UID");
        std::env::remove_var("PISTACK_SHELF_SQUASH_UID");
        let rw = export_line("/srv/share-1", "10.1.0.0/24", AccessMode::Write);
        assert!(rw.starts_with("/srv/share-1 10.1.0.0/24(rw,"), "{rw}");
        let ro = export_line("/srv/share-1", "10.0.0.0/24", AccessMode::Read);
        assert!(ro.starts_with("/srv/share-1 10.0.0.0/24(ro,"), "{ro}");
        assert!(!cidr_ok("../x"));
        assert!(!cidr_ok("10.0.0.0/33"));
        assert!(cidr_ok("10.0.0.0/24"));
        assert!(safe_id("..").is_err());
        match prev {
            Some(v) => std::env::set_var("PISTACK_SHELF_SQUASH_UID", v),
            None => std::env::remove_var("PISTACK_SHELF_SQUASH_UID"),
        }
    }

    #[tokio::test]
    async fn a_write_share_is_exported_rw_and_a_read_share_ro() {
        // The export line is what a client is handed. These are its own
        // tests: unit tests run side by side and one process cannot hold
        // two shelf roots, so the managed-file assertions live with the
        // end-to-end suites in `tests/shares.rs`.
        let _guard = share_dir::test_env_lock().lock().unwrap();
        let prev = std::env::var_os("PISTACK_SHELF_SQUASH_UID");
        std::env::remove_var("PISTACK_SHELF_SQUASH_UID");
        let ro = export_line("/srv/share-1", "10.0.0.0/24", AccessMode::Read);
        assert_eq!(
            ro,
            "/srv/share-1 10.0.0.0/24(ro,sync,no_subtree_check,\
             all_squash,anonuid=65534,anongid=65534)\n"
        );
        let rw = export_line("/srv/share-1", "10.1.0.0/24", AccessMode::Write);
        assert_eq!(
            rw,
            "/srv/share-1 10.1.0.0/24(rw,sync,no_subtree_check,\
             all_squash,anonuid=65534,anongid=65534)\n"
        );
        match prev {
            Some(v) => std::env::set_var("PISTACK_SHELF_SQUASH_UID", v),
            None => std::env::remove_var("PISTACK_SHELF_SQUASH_UID"),
        }
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn a_share_directory_is_owned_by_the_squashed_uid_and_setgid() {
        let _guard = env_lock().lock().await;
        let dir = scratch();
        let shelf = dir.join("shares");
        let exports = dir.join("pistack.exports");
        let ok = script(&dir, "exportfs", "#!/bin/sh\nexit 0\n");
        let _shelf = Env::set("PISTACK_SHELF_DIR", &shelf);
        let _file = Env::set("PISTACK_EXPORTS_FILE", &exports);
        let _euid = Env::set("PISTACK_SHELF_EUID", "0");
        let _helper = Env::unset("PISTACK_SHELF_HELPER");
        let _bin = Env::set("PISTACK_EXPORTFS", &ok);
        // The uid this process certainly owns: chowning to it needs no root,
        // so the owner and mode assertions run in an unprivileged test run.
        let me = share_dir::euid();
        let _uid = Env::set("PISTACK_SHELF_SQUASH_UID", me.to_string());

        let mut share = sample_mode(PROTOCOL_NFS, Some("10.2.0.0/24"), AccessMode::Write);
        share.meta.name = "owned".into();
        apply(&mut share).await.unwrap();
        assert_eq!(share.status.phase, Phase::Ready);
        let path = PathBuf::from(&share.status.message);
        let (uid, mode) = share_dir::probe(&path).expect("probe share dir");
        assert_eq!(uid, me, "share dir uid {uid}");
        assert_eq!(mode, share_dir::SHARE_DIR_MODE, "{mode:#o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn an_existing_share_directory_is_fixed_on_the_next_reconcile() {
        let _guard = env_lock().lock().await;
        let dir = scratch();
        let shelf = dir.join("shares");
        let exports = dir.join("pistack.exports");
        let ok = script(&dir, "exportfs", "#!/bin/sh\nexit 0\n");
        let _shelf = Env::set("PISTACK_SHELF_DIR", &shelf);
        let _file = Env::set("PISTACK_EXPORTS_FILE", &exports);
        let _euid = Env::set("PISTACK_SHELF_EUID", "0");
        let _helper = Env::unset("PISTACK_SHELF_HELPER");
        let _bin = Env::set("PISTACK_EXPORTFS", &ok);
        let me = share_dir::euid();
        let _uid = Env::set("PISTACK_SHELF_SQUASH_UID", me.to_string());

        let mut share = sample_mode(PROTOCOL_NFS, Some("10.3.0.0/24"), AccessMode::Write);
        share.meta.name = "fixed".into();
        apply(&mut share).await.unwrap();
        assert_eq!(share.status.phase, Phase::Ready);
        let path = PathBuf::from(&share.status.message);

        // An older pass left it this process's own, mode 0700, setgid gone:
        // the shape of the fault.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        let (uid, mode) = share_dir::probe(&path).expect("probe");
        assert_eq!(uid, me);
        assert_eq!(mode, 0o700, "{mode:#o}");

        // A pass that finds nothing else due — and a Ready share whose
        // revision is observed is nothing else due.
        reconcile_one(&share).await;
        let (uid, mode) = share_dir::probe(&path).expect("probe");
        assert_eq!(uid, me, "share dir uid {uid}");
        assert_eq!(mode, share_dir::SHARE_DIR_MODE, "{mode:#o}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One pass of the test-only reconciler over `share` as it stands in the
    /// shelf directory, through the same `drive` the server loops on.
    #[cfg(unix)]
    async fn reconcile_one(share: &Share) {
        let _guard = export_lock().lock().await;
        let mut all = vec![share.clone()];
        // `due_now` false: what a Ready share at its observed revision is on
        // a quiet tick. The pass must still walk the directory it exports.
        let _ = drive(&mut all, Utc::now(), None, false).await;
    }

    #[tokio::test]
    async fn a_ready_nfs_share_reports_the_address_to_mount() {
        let _guard = env_lock().lock().await;
        let dir = scratch();
        let shelf = dir.join("shares");
        let exports = dir.join("pistack.exports");
        let ok = script(&dir, "exportfs", "#!/bin/sh\nexit 0\n");
        let _shelf = Env::set("PISTACK_SHELF_DIR", &shelf);
        let _file = Env::set("PISTACK_EXPORTS_FILE", &exports);
        let _euid = Env::set("PISTACK_SHELF_EUID", "0");
        let _helper = Env::unset("PISTACK_SHELF_HELPER");
        let _bin = Env::set("PISTACK_EXPORTFS", &ok);
        let _uid = Env::unset("PISTACK_SHELF_SQUASH_UID");
        export_address::clear();
        std::env::set_var("PISTACK_EXPORT_ADDRESS", "10.97.0.11");
        // A unit test is not a module start: pretend the server started with
        // this address, the way `bin/pistack` starts one that has it set.
        export_address::set("10.97.0.11");

        let mut share = sample_mode(PROTOCOL_NFS, Some("10.4.0.0/24"), AccessMode::Write);
        share.meta.name = "mounted".into();
        apply(&mut share).await.unwrap();
        assert_eq!(share.status.phase, Phase::Ready);
        assert_eq!(
            share.status.export.as_deref(),
            Some(format!("10.97.0.11:{}", share.status.message).as_str())
        );
        // The local path stays where it always was.
        assert!(share.status.message.contains(&share.meta.id.to_string()));

        // Without an address the field is simply absent, as before: a start
        // that names no host, and no variable either.
        export_address::set("");
        let mut plain = sample_mode(PROTOCOL_NFS, Some("10.6.0.0/24"), AccessMode::Write);
        plain.meta.name = "plain".into();
        apply(&mut plain).await.unwrap();
        assert_eq!(plain.status.phase, Phase::Ready);
        assert_eq!(plain.status.export, None);

        // A cephfs share has no NFS address to give.
        let _fs = Env::unset("PISTACK_CEPH_FS");
        let mut ceph = sample(PROTOCOL_CEPHFS, None);
        apply(&mut ceph).await.unwrap();
        assert_eq!(ceph.status.phase, Phase::Pending);
        assert_eq!(ceph.status.export, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let _guard = env_lock().blocking_lock();
        let prev = std::env::var_os("PISTACK_SHELF_BACKOFF_MS");
        std::env::set_var("PISTACK_SHELF_BACKOFF_MS", "1000");
        assert_eq!(backoff_ms(1), 1_000);
        assert_eq!(backoff_ms(2), 2_000);
        assert_eq!(backoff_ms(3), 4_000);
        assert_eq!(backoff_ms(10), 300_000);
        match prev {
            Some(v) => std::env::set_var("PISTACK_SHELF_BACKOFF_MS", v),
            None => std::env::remove_var("PISTACK_SHELF_BACKOFF_MS"),
        }
    }

    #[test]
    fn share_root_prefers_shelf_dir_then_data_dir() {
        let _guard = env_lock().blocking_lock();
        let shelf = std::env::var_os("PISTACK_SHELF_DIR");
        let data = std::env::var_os("PISTACK_DATA_DIR");
        std::env::set_var("PISTACK_SHELF_DIR", "/tmp/shelf-a");
        std::env::set_var("PISTACK_DATA_DIR", "/tmp/data-a");
        assert_eq!(shelf_root(), PathBuf::from("/tmp/shelf-a"));
        std::env::remove_var("PISTACK_SHELF_DIR");
        assert_eq!(shelf_root(), PathBuf::from("/tmp/data-a/shares"));
        std::env::remove_var("PISTACK_DATA_DIR");
        assert_eq!(shelf_root(), PathBuf::from("./data/shares"));
        restore("PISTACK_SHELF_DIR", shelf);
        restore("PISTACK_DATA_DIR", data);
    }

    fn restore(key: &str, prev: Option<std::ffi::OsString>) {
        match prev {
            Some(v) => std::env::set_var(key, v),
            None => std::env::remove_var(key),
        }
    }

    fn scratch() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "pistack-shelf-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn script(dir: &Path, name: &str, body: &str) -> PathBuf {
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_mode(0o755);
            std::fs::set_permissions(&path, perms).unwrap();
        }
        path
    }

    fn sample(protocol: &str, cidr: Option<&str>) -> Share {
        sample_mode(protocol, cidr, AccessMode::Read)
    }

    fn sample_mode(protocol: &str, cidr: Option<&str>, mode: AccessMode) -> Share {
        use crate::types::ShareSpec;
        use pistack_types::{kind, Meta};
        let mut access = Vec::new();
        if let Some(cidr) = cidr {
            access.push(crate::types::ShareAccess {
                principal: None,
                cidr: Some(cidr.to_owned()),
                mode,
            });
        }
        Share {
            meta: Meta::new(kind::SHARE, "home", None),
            spec: ShareSpec {
                name: "home".into(),
                protocol: protocol.into(),
                size_gib: 1,
                access,
            },
            status: ShareStatus::default(),
        }
    }

    struct Env {
        key: &'static str,
        prev: Option<std::ffi::OsString>,
    }

    impl Env {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            std::env::set_var(key, value);
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var_os(key);
            std::env::remove_var(key);
            Self { key, prev }
        }
    }

    impl Drop for Env {
        fn drop(&mut self) {
            restore(self.key, self.prev.take());
        }
    }

    #[tokio::test]
    async fn nfs_is_ready_only_after_exportfs() {
        let _guard = env_lock().lock().await;
        let dir = scratch();
        let shelf = dir.join("shares");
        let log = dir.join("exportfs.log");
        let exports = dir.join("pistack.exports");
        let ok = script(
            &dir,
            "exportfs",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_EXPORTFS_LOG\"\nexit 0\n",
        );
        let fail = script(&dir, "exportfs-fail", "#!/bin/sh\necho boom >&2\nexit 1\n");
        let _shelf = Env::set("PISTACK_SHELF_DIR", &shelf);
        let _elog = Env::set("PISTACK_EXPORTFS_LOG", &log);
        let _file = Env::set("PISTACK_EXPORTS_FILE", &exports);
        let _euid = Env::set("PISTACK_SHELF_EUID", "0");
        let _helper = Env::unset("PISTACK_SHELF_HELPER");
        let _uid = Env::set("PISTACK_SHELF_SQUASH_UID", "65534");
        let _bin = Env::set("PISTACK_EXPORTFS", dir.join("missing-exportfs"));

        let mut pending = sample(PROTOCOL_NFS, Some("10.0.0.0/24"));
        apply(&mut pending).await.unwrap();
        assert_eq!(pending.status.phase, Phase::Error);
        assert_eq!(pending.status.message, "exportfs is not installed");
        assert!(pending.status.retry_at.is_some());

        std::env::set_var("PISTACK_EXPORTFS", &fail);
        let mut failed = sample(PROTOCOL_NFS, Some("10.0.1.0/24"));
        failed.meta.name = "home-fail".into();
        apply(&mut failed).await.unwrap();
        assert_eq!(failed.status.phase, Phase::Error);
        assert!(
            failed.status.message.contains("boom"),
            "{}",
            failed.status.message
        );

        std::env::set_var("PISTACK_EXPORTFS", &ok);
        let mut ready = sample(PROTOCOL_NFS, Some("10.4.0.0/24"));
        ready.meta.name = "home-ok".into();
        apply(&mut ready).await.unwrap();
        assert_eq!(ready.status.phase, Phase::Ready);
        assert!(ready.status.message.contains(&ready.meta.id.to_string()));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("-ra"), "{text}");
        let file = std::fs::read_to_string(&exports).unwrap();
        assert!(
            file.contains(
                "10.4.0.0/24(ro,sync,no_subtree_check,all_squash,anonuid=65534,anongid=65534)"
            ),
            "{file}"
        );
        assert!(file.starts_with("# Managed by pistack shelf"), "{file}");

        remove(&ready).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert_eq!(text.lines().filter(|l| *l == "-ra").count(), 2, "{text}");
        let file = std::fs::read_to_string(&exports).unwrap();
        assert!(!file.contains("10.4.0.0/24"), "{file}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ceph_stays_pending_until_the_binary_creates_a_subvolume() {
        let _guard = env_lock().lock().await;
        let dir = scratch();
        let log = dir.join("ceph.log");
        let bin = script(
            &dir,
            "ceph",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_CEPH_LOG\"\nexit 0\n",
        );
        let _bin = Env::set("PISTACK_CEPH_BIN", dir.join("missing-ceph"));
        let _fs = Env::unset("PISTACK_CEPH_FS");
        let _log = Env::unset("PISTACK_CEPH_LOG");

        let mut missing = sample(PROTOCOL_CEPHFS, None);
        apply(&mut missing).await.unwrap();
        assert_eq!(missing.status.phase, Phase::Pending);
        assert_eq!(missing.status.message, NO_CEPH);

        std::env::set_var("PISTACK_CEPH_BIN", &bin);
        let mut no_fs = sample(PROTOCOL_CEPHFS, None);
        apply(&mut no_fs).await.unwrap();
        assert_eq!(no_fs.status.phase, Phase::Pending);
        assert_eq!(no_fs.status.message, NO_CEPH_FS);

        std::env::set_var("PISTACK_CEPH_FS", "pistack");
        std::env::set_var("PISTACK_CEPH_LOG", &log);
        let mut ready = sample(PROTOCOL_CEPHFS, None);
        ready.spec.size_gib = 2;
        apply(&mut ready).await.unwrap();
        assert_eq!(ready.status.phase, Phase::Ready);
        assert_eq!(ready.status.message, format!("pistack/{}", ready.meta.id));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("subvolume"), "{text}");
        assert!(text.contains("create"), "{text}");
        assert!(text.contains("pistack"), "{text}");
        assert!(text.contains(&ready.meta.id.to_string()), "{text}");
        assert!(text.contains("2147483648"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
