// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Turn an NFS share into a directory, an exports fragment, and a live
//! export. CephFS creates a subvolume when `ceph` is configured.
//!
//! The directory is `$PISTACK_SHELF_DIR/<share-id>` (default
//! `/var/lib/pistack/shares`). Each client CIDR becomes one line in
//! `<shelf>/exports.d/<id>.exports`:
//! `<dir> <cidr>(rw|ro,sync,no_subtree_check)`. `exportfs` (or
//! `$PISTACK_EXPORTFS`) must accept that client before the share is
//! `Ready`. A missing or failing tool stays `Pending`. Paths outside the
//! shelf directory are refused.

use std::path::{Path, PathBuf};

use pistack_types::{ApiError, ApiResult, Phase};

use crate::types::{AccessMode, Share, ShareStatus, PROTOCOL_CEPHFS, PROTOCOL_NFS};

const DEFAULT_ROOT: &str = "/var/lib/pistack/shares";
pub const NO_CEPH: &str = "ceph is not installed";
pub const NO_CEPH_FS: &str = "ceph fs name is not configured";
pub const NO_CIDR: &str = "nfs share has no client cidr";

fn shelf_root() -> PathBuf {
    match std::env::var_os("PISTACK_SHELF_DIR") {
        Some(v) if !v.is_empty() => PathBuf::from(v),
        _ => PathBuf::from(DEFAULT_ROOT),
    }
}

/// Create or refresh the on-disk share and set `status`.
pub async fn apply(share: &mut Share) -> ApiResult<()> {
    let id = share.meta.id.to_string();
    safe_id(&id)?;
    match share.spec.protocol.as_str() {
        PROTOCOL_CEPHFS => {
            remove_exports(&id).await?;
            realize_ceph(share, &id).await
        }
        PROTOCOL_NFS => realize_nfs(share, &id).await,
        other => Err(ApiError::bad_request(format!(
            "unknown protocol '{other}'"
        ))),
    }
}

/// Remove the directory and exports file created for `share`. Idempotent.
pub async fn remove(share: &Share) -> ApiResult<()> {
    let id = share.meta.id.to_string();
    safe_id(&id)?;
    let Some(root) = existing_root().await? else {
        return Ok(());
    };
    let dir = root.join(&id);
    if share.spec.protocol == PROTOCOL_NFS {
        unexport(&root, &dir, share).await;
    }
    remove_dir_inside(&root, &dir).await?;
    if let Some(path) = exports_path(&root, &id)? {
        remove_file_inside(&root, &path).await?;
    }
    if share.spec.protocol == PROTOCOL_CEPHFS {
        remove_ceph(share, &id).await?;
    }
    Ok(())
}

async fn realize_nfs(share: &mut Share, id: &str) -> ApiResult<()> {
    let rules: Vec<(String, AccessMode)> = share
        .spec
        .access
        .iter()
        .filter_map(|rule| {
            rule.cidr
                .as_ref()
                .map(|cidr| (cidr.clone(), rule.mode))
        })
        .collect();
    if rules.is_empty() {
        remove_exports(id).await?;
        share.status = ShareStatus {
            phase: Phase::Pending,
            message: NO_CIDR.to_owned(),
        };
        return Ok(());
    }
    for (cidr, _) in &rules {
        if !cidr_ok(cidr) {
            return Err(ApiError::bad_request(format!(
                "access cidr '{cidr}' is not a CIDR"
            )));
        }
    }
    let root = canonical_root().await?;
    let dir = root.join(id);
    if !dir.starts_with(&root) {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    if std::fs::symlink_metadata(&dir)
        .ok()
        .is_some_and(|m| m.file_type().is_symlink())
    {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;
    let canon = tokio::fs::canonicalize(&dir).await.map_err(io_err)?;
    if !canon.starts_with(&root) || canon == root {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    let dir_s = canon.to_string_lossy().into_owned();
    let mut body = String::new();
    for (cidr, mode) in &rules {
        body.push_str(&export_line(&dir_s, cidr, *mode));
    }
    let exports_dir = root.join("exports.d");
    tokio::fs::create_dir_all(&exports_dir)
        .await
        .map_err(io_err)?;
    let Some(path) = exports_path(&root, id)? else {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    };
    if let Err(e) = tokio::fs::write(&path, &body).await {
        return Err(io_err(e));
    }
    let Some(bin) = tool_bin("PISTACK_EXPORTFS", "exportfs") else {
        share.status = ShareStatus {
            phase: Phase::Pending,
            message: "exportfs is not installed".to_owned(),
        };
        return Ok(());
    };
    for (cidr, mode) in &rules {
        let flag = match mode {
            AccessMode::Read => "ro",
            AccessMode::Write => "rw",
        };
        let opts = format!("{flag},sync,no_subtree_check");
        let spec = format!("{cidr}:{dir_s}");
        if let Err(message) = run_tool(&bin, &["-o", &opts, &spec]).await {
            share.status = ShareStatus {
                phase: Phase::Pending,
                message,
            };
            return Ok(());
        }
    }
    share.status = ShareStatus {
        phase: Phase::Ready,
        message: dir_s,
    };
    Ok(())
}

async fn realize_ceph(share: &mut Share, id: &str) -> ApiResult<()> {
    let Some(bin) = tool_bin("PISTACK_CEPH_BIN", "ceph") else {
        share.status = ShareStatus {
            phase: Phase::Pending,
            message: NO_CEPH.to_owned(),
        };
        return Ok(());
    };
    let Some(fs) = ceph_fs() else {
        share.status = ShareStatus {
            phase: Phase::Pending,
            message: NO_CEPH_FS.to_owned(),
        };
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
        share.status = ShareStatus {
            phase: Phase::Pending,
            message,
        };
        return Ok(());
    }
    share.status = ShareStatus {
        phase: Phase::Ready,
        message: format!("{fs}/{id}"),
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
    let output = tokio::process::Command::new(bin)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                format!("{} is not installed", bin.display())
            } else {
                format!("{} failed to start: {e}", bin.display())
            }
        })?;
    if output.status.success() {
        return Ok(());
    }
    let err = String::from_utf8_lossy(&output.stderr);
    let out = String::from_utf8_lossy(&output.stdout);
    let detail = clip(&format!("{err} {out}"));
    let name = bin
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("tool");
    if detail.is_empty() {
        Err(format!("{name} failed"))
    } else {
        Err(format!("{name} failed: {detail}"))
    }
}

fn clip(s: &str) -> String {
    let flat: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = flat.trim();
    if trimmed.len() > 180 {
        format!("{}…", &trimmed[..180])
    } else {
        trimmed.to_owned()
    }
}

fn export_line(dir: &str, cidr: &str, mode: AccessMode) -> String {
    let flag = match mode {
        AccessMode::Read => "ro",
        AccessMode::Write => "rw",
    };
    format!("{dir} {cidr}({flag},sync,no_subtree_check)\n")
}

async fn unexport(root: &Path, dir: &Path, share: &Share) {
    let Ok(canon) = tokio::fs::canonicalize(dir).await else {
        return;
    };
    if !canon.starts_with(root) || canon == root {
        return;
    }
    let Some(bin) = tool_bin("PISTACK_EXPORTFS", "exportfs") else {
        return;
    };
    let dir_s = canon.to_string_lossy().into_owned();
    for rule in &share.spec.access {
        let Some(cidr) = &rule.cidr else {
            continue;
        };
        let spec = format!("{cidr}:{dir_s}");
        let _ = run_tool(&bin, &["-u", &spec]).await;
    }
}

async fn remove_exports(id: &str) -> ApiResult<()> {
    let Some(root) = existing_root().await? else {
        return Ok(());
    };
    if let Some(path) = exports_path(&root, id)? {
        remove_file_inside(&root, &path).await?;
    }
    Ok(())
}

async fn canonical_root() -> ApiResult<PathBuf> {
    let root = shelf_root();
    tokio::fs::create_dir_all(&root).await.map_err(io_err)?;
    tokio::fs::canonicalize(&root).await.map_err(io_err)
}

async fn existing_root() -> ApiResult<Option<PathBuf>> {
    let root = shelf_root();
    match tokio::fs::try_exists(&root).await {
        Ok(false) => Ok(None),
        Ok(true) => Ok(Some(tokio::fs::canonicalize(&root).await.map_err(io_err)?)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(io_err(e)),
    }
}

fn exports_path(root: &Path, id: &str) -> ApiResult<Option<PathBuf>> {
    let name = format!("{id}.exports");
    if !component_ok(&name) {
        return Ok(None);
    }
    let path = root.join("exports.d").join(&name);
    if path.starts_with(root) {
        Ok(Some(path))
    } else {
        Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ))
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
        Err(e) => return Err(io_err(e)),
    };
    if meta.file_type().is_symlink() {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    if !meta.is_dir() {
        tokio::fs::remove_file(dir).await.map_err(io_err)?;
        return Ok(());
    }
    let canon = tokio::fs::canonicalize(dir).await.map_err(io_err)?;
    if !canon.starts_with(root) || canon == root {
        return Err(ApiError::bad_request(
            "share path escapes the shelf directory",
        ));
    }
    tokio::fs::remove_dir_all(dir).await.map_err(io_err)?;
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
        Err(e) => return Err(io_err(e)),
    }
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(e)),
    }
}

fn io_err(e: std::io::Error) -> ApiError {
    ApiError::internal(format!("shelf: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn export_line_marks_read_and_write() {
        assert_eq!(
            export_line("/srv/share-1", "10.0.0.0/24", AccessMode::Read),
            "/srv/share-1 10.0.0.0/24(ro,sync,no_subtree_check)\n"
        );
        assert_eq!(
            export_line("/srv/share-1", "10.1.0.0/24", AccessMode::Write),
            "/srv/share-1 10.1.0.0/24(rw,sync,no_subtree_check)\n"
        );
        assert!(!cidr_ok("../x"));
        assert!(!cidr_ok("10.0.0.0/33"));
        assert!(cidr_ok("10.0.0.0/24"));
        assert!(safe_id("..").is_err());
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
        use crate::types::{ShareAccess, ShareSpec};
        use pistack_types::{kind, Meta};
        let mut access = Vec::new();
        if let Some(cidr) = cidr {
            access.push(ShareAccess {
                principal: None,
                cidr: Some(cidr.to_owned()),
                mode: AccessMode::Read,
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

    #[tokio::test]
    async fn nfs_is_ready_only_after_exportfs() {
        let dir = scratch();
        let shelf = dir.join("shares");
        let log = dir.join("exportfs.log");
        let ok = script(
            &dir,
            "exportfs",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_EXPORTFS_LOG\"\nexit 0\n",
        );
        let fail = script(&dir, "exportfs-fail", "#!/bin/sh\necho boom >&2\nexit 1\n");
        let _shelf = std::env::var("PISTACK_SHELF_DIR").ok();
        let _bin = std::env::var("PISTACK_EXPORTFS").ok();
        let _elog = std::env::var("PISTACK_EXPORTFS_LOG").ok();
        std::env::set_var("PISTACK_SHELF_DIR", &shelf);
        std::env::set_var("PISTACK_EXPORTFS_LOG", &log);
        std::env::set_var("PISTACK_EXPORTFS", dir.join("missing-exportfs"));

        let mut pending = sample(PROTOCOL_NFS, Some("10.0.0.0/24"));
        apply(&mut pending).await.unwrap();
        assert_eq!(pending.status.phase, Phase::Pending);
        assert_eq!(pending.status.message, "exportfs is not installed");

        std::env::set_var("PISTACK_EXPORTFS", &fail);
        let mut failed = sample(PROTOCOL_NFS, Some("10.0.0.0/24"));
        apply(&mut failed).await.unwrap();
        assert_eq!(failed.status.phase, Phase::Pending);
        assert!(failed.status.message.contains("boom"), "{}", failed.status.message);

        std::env::set_var("PISTACK_EXPORTFS", &ok);
        let mut ready = sample(PROTOCOL_NFS, Some("10.4.0.0/24"));
        apply(&mut ready).await.unwrap();
        assert_eq!(ready.status.phase, Phase::Ready);
        assert!(ready.status.message.contains(&ready.meta.id.to_string()));
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("-o"), "{text}");
        assert!(text.contains("10.4.0.0/24:"), "{text}");
        assert!(text.contains("ro,sync,no_subtree_check"), "{text}");

        remove(&ready).await.unwrap();
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("-u"), "{text}");

        match _shelf {
            Some(v) => std::env::set_var("PISTACK_SHELF_DIR", v),
            None => std::env::remove_var("PISTACK_SHELF_DIR"),
        }
        match _bin {
            Some(v) => std::env::set_var("PISTACK_EXPORTFS", v),
            None => std::env::remove_var("PISTACK_EXPORTFS"),
        }
        match _elog {
            Some(v) => std::env::set_var("PISTACK_EXPORTFS_LOG", v),
            None => std::env::remove_var("PISTACK_EXPORTFS_LOG"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn ceph_stays_pending_until_the_binary_creates_a_subvolume() {
        let dir = scratch();
        let log = dir.join("ceph.log");
        let bin = script(
            &dir,
            "ceph",
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> \"$PISTACK_CEPH_LOG\"\nexit 0\n",
        );
        let _bin = std::env::var("PISTACK_CEPH_BIN").ok();
        let _fs = std::env::var("PISTACK_CEPH_FS").ok();
        let _log = std::env::var("PISTACK_CEPH_LOG").ok();
        std::env::set_var("PISTACK_CEPH_BIN", dir.join("missing-ceph"));
        std::env::remove_var("PISTACK_CEPH_FS");

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
        assert_eq!(
            ready.status.message,
            format!("pistack/{}", ready.meta.id)
        );
        let text = std::fs::read_to_string(&log).unwrap();
        assert!(text.contains("subvolume"), "{text}");
        assert!(text.contains("create"), "{text}");
        assert!(text.contains("pistack"), "{text}");
        assert!(text.contains(&ready.meta.id.to_string()), "{text}");
        assert!(text.contains("2147483648"), "{text}");

        match _bin {
            Some(v) => std::env::set_var("PISTACK_CEPH_BIN", v),
            None => std::env::remove_var("PISTACK_CEPH_BIN"),
        }
        match _fs {
            Some(v) => std::env::set_var("PISTACK_CEPH_FS", v),
            None => std::env::remove_var("PISTACK_CEPH_FS"),
        }
        match _log {
            Some(v) => std::env::set_var("PISTACK_CEPH_LOG", v),
            None => std::env::remove_var("PISTACK_CEPH_LOG"),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
