// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Mount a slab volume as a directory for a pod.
//!
//! A pod attach turns the block device into a filesystem and mounts it at
//! `$PISTACK_DATA_DIR/slab/mounts/<volume-id>` (or `$PISTACK_VOLFS_DATA_DIR`
//! when that is set). The agent records that directory as the host path.
//! A VM attach does not come here.
//!
//! A local image file is attached with `losetup --find --show`. An LVM
//! device node and an RBD device mapped by `rbdmap` are used as they are.
//! `blkid -p -o value -s TYPE` probes the filesystem. Empty output formats
//! with `mkfs.ext4 -q -F`, or `mkfs.<fs_type>` when the volume type sets
//! `fs_type`. A device that already has a filesystem is never formatted.
//!
//! Detach unmounts that directory, then `losetup -d` when a loop device
//! was used. An unmount that reports the directory is not mounted is
//! success. The caller then runs the existing LVM or RBD detach.
//!
//! A root agent runs the tools directly. `PISTACK_VOLFS_SUDO` does not
//! change that. When the agent is not root and `PISTACK_VOLFS_SUDO` is
//! `1`, `true`, or `yes`, `losetup`, `blkid`, `mkfs`, `mount`, and `umount`
//! run as `sudo -n`. `sudo -n` fails closed without a rule. Allow only
//! those binaries:
//!
//! ```text
//! # /etc/sudoers.d/pistack-volfs
//! ubuntu ALL=(root) NOPASSWD: /sbin/losetup, /sbin/blkid, /sbin/mkfs.ext4, /bin/mount, /bin/umount
//! ```
//!
//! Replace `ubuntu` and the paths with the agent user and the binaries on
//! the host. A volume type that sets `fs_type` to something other than
//! `ext4` needs that `mkfs` binary on the same rule.
//! `$PISTACK_LOSETUP`, `$PISTACK_BLKID`, `$PISTACK_MKFS`, `$PISTACK_MOUNT`,
//! and `$PISTACK_UMOUNT` replace those binaries. `$PISTACK_VOLFS_SUDO_BIN`
//! replaces `sudo` for tests.

use pistack_rpc::proto::DiskTarget;
use pistack_types::{ApiError, ApiResult, Id};
use std::path::{Component, Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use tokio::process::Command;

use super::VolumeAttach;

const PROBE_TIMEOUT: Duration = Duration::from_secs(30);
const MKFS_TIMEOUT: Duration = Duration::from_secs(120);
const LOSETUP_ENV: &str = "PISTACK_LOSETUP";
const BLKID_ENV: &str = "PISTACK_BLKID";
const MKFS_ENV: &str = "PISTACK_MKFS";
const MOUNT_ENV: &str = "PISTACK_MOUNT";
const UMOUNT_ENV: &str = "PISTACK_UMOUNT";
const SUDO_ENV: &str = "PISTACK_VOLFS_SUDO";
const SUDO_BIN_ENV: &str = "PISTACK_VOLFS_SUDO_BIN";
const DATA_ENV: &str = "PISTACK_VOLFS_DATA_DIR";
const SUDOERS_SAMPLE: &str = "ubuntu ALL=(root) NOPASSWD: /sbin/losetup, /sbin/blkid, /sbin/mkfs.ext4, /bin/mount, /bin/umount";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DiskKind {
    /// Local image file. `losetup` yields the block device.
    File,
    /// LVM device node or mapped RBD device.
    Device,
}

/// Where a pod mount is planned, before any command runs.
#[cfg(test)]
struct MountPlan<'a> {
    kind: DiskKind,
    source: &'a str,
    /// Device `losetup --find --show` printed. Ignored for [`DiskKind::Device`].
    loop_device: &'a str,
    mount_dir: &'a str,
    fs_type: &'a str,
    has_filesystem: bool,
    read_only: bool,
}

/// Mount `req` for a pod and return the directory to record as the host path.
///
/// `recorded` is the path youki stored. For an RBD volume that is the mapped
/// device. On failure the loop device is detached and any mount is removed.
/// The caller still unmaps RBD.
pub(crate) async fn mount_for_pod(req: &VolumeAttach, recorded: &str) -> ApiResult<String> {
    let fs = canonical_fs(&req.fs_type)?;
    let (dir, sidecar) = volume_paths(&req.volume)?;
    // A previous attempt may have left a loop device or an empty directory.
    release_volume(&req.volume).await?;
    let kind = disk_kind(req, recorded);
    let source = block_source(req, recorded, kind);
    if !command_path_ok(source) {
        return Err(ApiError::bad_request(format!(
            "volume source {source} must be an absolute path"
        )));
    }
    let device = match kind {
        DiskKind::File => {
            let found = losetup_find(source).await?;
            if let Err(err) = write_sidecar(&sidecar, &found) {
                if let Err(undo) = detach_loop_idempotent(&found).await {
                    tracing::warn!(
                        volume = %req.volume,
                        device = %found,
                        error = %undo,
                        "anvil: detaching a loop device after a failed sidecar write"
                    );
                }
                return Err(err);
            }
            found
        }
        DiskKind::Device => source.to_owned(),
    };
    let probe = match filesystem_type(&device).await {
        Ok(found) => found,
        Err(err) => {
            rollback(&req.volume).await;
            return Err(err);
        }
    };
    if probe.is_none() {
        if req.read_only {
            rollback(&req.volume).await;
            return Err(ApiError::unavailable(
                "volume has no filesystem and cannot be formatted while read only",
            ));
        }
        if let Err(err) = make_filesystem(&fs, &device).await {
            rollback(&req.volume).await;
            return Err(err);
        }
    }
    if let Err(err) = prepare_mount_dir(&dir) {
        rollback(&req.volume).await;
        return Err(err);
    }
    if let Err(err) = mount_device(&device, &dir, req.read_only).await {
        rollback(&req.volume).await;
        return Err(err);
    }
    if !dir.is_dir() {
        rollback(&req.volume).await;
        return Err(ApiError::unavailable(format!(
            "mount directory {} is not a directory",
            dir.display()
        )));
    }
    let host_path = dir.to_string_lossy().into_owned();
    tracing::info!(
        volume = %req.volume,
        host_path = %host_path,
        filesystem = %probe.as_deref().unwrap_or(fs.as_str()),
        "anvil: mounted slab volume for pod"
    );
    Ok(host_path)
}

/// Unmount the pod directory for `volume` and detach its loop device.
///
/// Missing mount and an already-detached loop are success. A device that
/// is still mounted returns the `umount` error so the caller does not
/// unmap it.
pub(crate) async fn release_volume(volume: &Id) -> ApiResult<()> {
    let (dir, sidecar) = volume_paths(volume)?;
    if !dir.exists() && !sidecar.exists() {
        return Ok(());
    }
    if dir.exists() {
        unmount_idempotent(&dir).await?;
    }
    if let Some(device) = read_sidecar(&sidecar)? {
        detach_loop_idempotent(&device).await?;
    }
    if sidecar.exists() {
        std::fs::remove_file(&sidecar).map_err(|err| io_err("removing", &sidecar, err))?;
    }
    if dir.exists() {
        // The mount point is empty once the filesystem is gone.
        let _ = std::fs::remove_dir(&dir);
    }
    Ok(())
}

async fn rollback(volume: &Id) {
    if let Err(err) = release_volume(volume).await {
        tracing::warn!(
            volume = %volume,
            error = %err,
            "anvil: rolling back a slab mount failed"
        );
    }
}

fn disk_kind(req: &VolumeAttach, recorded: &str) -> DiskKind {
    if matches!(req.target, Some(DiskTarget::Rbd { .. })) && !recorded.is_empty() {
        return DiskKind::Device;
    }
    let path = Path::new(&req.path);
    if is_block_device(path) {
        return DiskKind::Device;
    }
    if req.path.starts_with("/dev/") && !is_regular_file(path) {
        return DiskKind::Device;
    }
    DiskKind::File
}

fn block_source<'a>(req: &'a VolumeAttach, recorded: &'a str, kind: DiskKind) -> &'a str {
    if kind == DiskKind::Device && matches!(req.target, Some(DiskTarget::Rbd { .. })) {
        recorded
    } else {
        req.path.as_str()
    }
}

#[cfg(test)]
fn attach_plan(plan: &MountPlan<'_>) -> ApiResult<Vec<Vec<String>>> {
    let mut steps = Vec::new();
    let device = if plan.kind == DiskKind::File {
        steps.push(argv("losetup", &losetup_find_args(plan.source)));
        plan.loop_device
    } else {
        plan.source
    };
    steps.push(argv("blkid", &blkid_args(device)));
    if !plan.has_filesystem && !plan.read_only {
        let (bin, args) = mkfs_command(plan.fs_type, device)?;
        steps.push(argv(&bin, &args));
    }
    steps.push(argv(
        "mount",
        &mount_args(device, plan.mount_dir, plan.read_only),
    ));
    Ok(steps)
}

#[cfg(test)]
fn detach_plan(mount_dir: &str, loop_device: Option<&str>) -> Vec<Vec<String>> {
    let mut steps = vec![argv("umount", &umount_args(mount_dir))];
    if let Some(device) = loop_device {
        steps.push(argv("losetup", &losetup_detach_args(device)));
    }
    steps
}

#[cfg(test)]
fn argv(program: &str, args: &[String]) -> Vec<String> {
    let mut step = Vec::with_capacity(args.len() + 1);
    step.push(program.to_owned());
    step.extend(args.iter().cloned());
    step
}

fn losetup_find_args(file: &str) -> Vec<String> {
    vec!["--find".to_owned(), "--show".to_owned(), file.to_owned()]
}

fn losetup_detach_args(device: &str) -> Vec<String> {
    vec!["-d".to_owned(), device.to_owned()]
}

fn blkid_args(device: &str) -> Vec<String> {
    vec![
        "-p".to_owned(),
        "-o".to_owned(),
        "value".to_owned(),
        "-s".to_owned(),
        "TYPE".to_owned(),
        device.to_owned(),
    ]
}

fn mount_args(device: &str, dir: &str, read_only: bool) -> Vec<String> {
    let mut args = Vec::new();
    if read_only {
        args.push("-o".to_owned());
        args.push("ro".to_owned());
    }
    args.push(device.to_owned());
    args.push(dir.to_owned());
    args
}

fn umount_args(dir: &str) -> Vec<String> {
    vec![dir.to_owned()]
}

/// `mkfs.<fs>` and the arguments that keep it quiet and non-interactive.
fn mkfs_command(fs: &str, device: &str) -> ApiResult<(String, Vec<String>)> {
    let fs = canonical_fs(fs)?;
    let program = format!("mkfs.{fs}");
    let mut args = match fs.as_str() {
        "xfs" => vec!["-f".to_owned(), "-q".to_owned()],
        "btrfs" => vec!["-f".to_owned()],
        _ => vec!["-q".to_owned(), "-F".to_owned()],
    };
    args.push(device.to_owned());
    Ok((program, args))
}

fn canonical_fs(raw: &str) -> ApiResult<String> {
    let trimmed = raw.trim();
    let fs = if trimmed.is_empty() { "ext4" } else { trimmed };
    let fs = fs.to_ascii_lowercase();
    let bytes = fs.as_bytes();
    let ok = (2..=8).contains(&fs.len())
        && bytes[0].is_ascii_lowercase()
        && bytes
            .iter()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit());
    if ok {
        Ok(fs)
    } else {
        Err(ApiError::bad_request(format!(
            "filesystem type {raw} is not supported"
        )))
    }
}

async fn losetup_find(file: &str) -> ApiResult<String> {
    let bin = tool_program(LOSETUP_ENV, "losetup");
    let output = exec(&bin, &losetup_find_args(file), PROBE_TIMEOUT).await?;
    if !output.status.success() {
        return Err(command_failed("losetup", &output));
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let device = text.split_whitespace().next().unwrap_or("").trim();
    if !loop_device_ok(device) {
        return Err(ApiError::unavailable(
            "losetup did not return a loop device",
        ));
    }
    Ok(device.to_owned())
}

async fn filesystem_type(device: &str) -> ApiResult<Option<String>> {
    let bin = tool_program(BLKID_ENV, "blkid");
    let output = exec(&bin, &blkid_args(device), PROBE_TIMEOUT).await?;
    let found = first_token(&output.stdout);
    if output.status.success() {
        return Ok(found);
    }
    // blkid exits 2 when the device has no recognizable filesystem.
    if output.status.code() == Some(2) && found.is_none() {
        return Ok(None);
    }
    if found.is_some() {
        return Ok(found);
    }
    Err(command_failed("blkid", &output))
}

async fn make_filesystem(fs: &str, device: &str) -> ApiResult<()> {
    let (program, args) = mkfs_command(fs, device)?;
    let bin = tool_program(MKFS_ENV, &program);
    let output = exec(&bin, &args, MKFS_TIMEOUT).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed(&program, &output))
    }
}

async fn mount_device(device: &str, dir: &Path, read_only: bool) -> ApiResult<()> {
    let bin = tool_program(MOUNT_ENV, "mount");
    let dir = dir.to_string_lossy().into_owned();
    let output = exec(&bin, &mount_args(device, &dir, read_only), PROBE_TIMEOUT).await?;
    if output.status.success() {
        Ok(())
    } else {
        Err(command_failed("mount", &output))
    }
}

async fn unmount_idempotent(dir: &Path) -> ApiResult<()> {
    let bin = tool_program(UMOUNT_ENV, "umount");
    let output = exec(&bin, &umount_args(&dir.to_string_lossy()), PROBE_TIMEOUT).await?;
    if output.status.success() || already_unmounted(&output) {
        return Ok(());
    }
    Err(command_failed("umount", &output))
}

async fn detach_loop_idempotent(device: &str) -> ApiResult<()> {
    if !loop_device_ok(device) {
        return Err(ApiError::unavailable(format!(
            "loop device {device} cannot be detached"
        )));
    }
    let bin = tool_program(LOSETUP_ENV, "losetup");
    let output = exec(&bin, &losetup_detach_args(device), PROBE_TIMEOUT).await?;
    if output.status.success() || already_detached(&output) {
        return Ok(());
    }
    Err(command_failed("losetup", &output))
}

fn first_token(stdout: &[u8]) -> Option<String> {
    let text = String::from_utf8_lossy(stdout);
    let token = text.split_whitespace().next()?.trim();
    if token.is_empty() {
        None
    } else {
        Some(token.to_owned())
    }
}

fn already_unmounted(output: &std::process::Output) -> bool {
    if output.status.code() == Some(32) {
        return true;
    }
    let text = combined(output);
    text.contains("not mounted") || text.contains("no mount point")
}

fn already_detached(output: &std::process::Output) -> bool {
    let text = combined(output);
    text.contains("no such device")
        || text.contains("no such file")
        || text.contains("not a loop device")
}

fn combined(output: &std::process::Output) -> String {
    format!(
        "{} {}",
        String::from_utf8_lossy(&output.stderr),
        String::from_utf8_lossy(&output.stdout)
    )
    .to_ascii_lowercase()
}

async fn exec(bin: &Path, args: &[String], timeout: Duration) -> ApiResult<std::process::Output> {
    let elevate = want_sudo();
    let program = if elevate {
        sudo_program()?
    } else {
        bin.to_path_buf()
    };
    let mut cmd = Command::new(&program);
    if elevate {
        cmd.arg("-n").arg(bin);
    }
    cmd.args(args)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let child = cmd.spawn().map_err(|err| {
        ApiError::unavailable(format!(
            "{} is not installed ({}: {err}){}",
            bin.display(),
            program.display(),
            privilege_hint()
        ))
    })?;
    match tokio::time::timeout(timeout, child.wait_with_output()).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(err)) => Err(ApiError::unavailable(format!(
            "{} failed: {err}{}",
            bin.display(),
            privilege_hint()
        ))),
        Err(_) => Err(ApiError::unavailable(format!(
            "{} timed out after {}s{}",
            bin.display(),
            timeout.as_secs(),
            privilege_hint()
        ))),
    }
}

fn command_failed(name: &str, output: &std::process::Output) -> ApiError {
    ApiError::unavailable(format!(
        "{name} failed: {}{}",
        brief(output),
        privilege_hint()
    ))
}

fn brief(output: &std::process::Output) -> String {
    let err = String::from_utf8_lossy(&output.stderr);
    let err = err.trim();
    if !err.is_empty() {
        return clip(err);
    }
    let out = String::from_utf8_lossy(&output.stdout);
    let out = out.trim();
    if !out.is_empty() {
        return clip(out);
    }
    let code = output.status.code().unwrap_or(-1);
    format!("exit {code}")
}

fn clip(text: &str) -> String {
    const MAX: usize = 300;
    let flat = text.replace('\n', " ");
    if flat.chars().count() <= MAX {
        flat
    } else {
        flat.chars().take(MAX).collect()
    }
}

fn privilege_hint() -> String {
    if agent_is_root() || env_flag(SUDO_ENV) {
        String::new()
    } else {
        format!(" Set {SUDO_ENV}=1 and allow `{SUDOERS_SAMPLE}`.")
    }
}

fn tool_program(env_key: &str, default: &str) -> PathBuf {
    if let Ok(raw) = std::env::var(env_key) {
        let raw = raw.trim();
        if !raw.is_empty() && !raw.contains('\0') {
            return PathBuf::from(raw);
        }
    }
    PathBuf::from(default)
}

fn want_sudo() -> bool {
    !agent_is_root() && env_flag(SUDO_ENV)
}

fn env_flag(name: &str) -> bool {
    std::env::var(name)
        .map(|value| matches!(value.as_str(), "1" | "true" | "yes"))
        .unwrap_or(false)
}

fn agent_is_root() -> bool {
    #[cfg(test)]
    if let Ok(raw) = std::env::var("PISTACK_VOLFS_TEST_EUID") {
        if let Ok(uid) = raw.parse::<u32>() {
            return uid == 0;
        }
    }
    #[cfg(unix)]
    {
        // geteuid does not fail.
        unsafe { libc::geteuid() == 0 }
    }
    #[cfg(not(unix))]
    {
        true
    }
}

fn sudo_program() -> ApiResult<PathBuf> {
    if let Some(raw) = std::env::var_os(SUDO_BIN_ENV) {
        if !raw.is_empty() {
            let path = PathBuf::from(raw);
            if executable(&path) {
                return Ok(path);
            }
            return Err(ApiError::unavailable(format!(
                "slab mount: {SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
            )));
        }
    }
    crate::facts::which("sudo").ok_or_else(|| {
        ApiError::unavailable(format!(
            "slab mount: {SUDO_ENV} is set but sudo is not available. Install sudo and `{SUDOERS_SAMPLE}`"
        ))
    })
}

fn executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

fn volume_paths(volume: &Id) -> ApiResult<(PathBuf, PathBuf)> {
    let name = volume_name(volume)?;
    let root = data_dir()?.join("slab").join("mounts");
    Ok((root.join(&name), root.join(format!("{name}.loop"))))
}

fn volume_name(volume: &Id) -> ApiResult<String> {
    let name = volume.to_string();
    if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '-') {
        Ok(name)
    } else {
        Err(ApiError::bad_request(
            "volume id cannot name a mount directory",
        ))
    }
}

fn data_dir() -> ApiResult<PathBuf> {
    let raw = std::env::var_os(DATA_ENV)
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("PISTACK_DATA_DIR").filter(|value| !value.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/var/lib/pistack"));
    if raw.is_absolute() && !raw.components().any(|c| matches!(c, Component::ParentDir)) {
        Ok(raw)
    } else {
        Err(ApiError::bad_request(
            "slab mount data dir must be an absolute path",
        ))
    }
}

fn prepare_mount_dir(dir: &Path) -> ApiResult<()> {
    if let Ok(meta) = std::fs::symlink_metadata(dir) {
        if meta.file_type().is_symlink() {
            return Err(ApiError::unavailable(format!(
                "mount directory {} is a symlink",
                dir.display()
            )));
        }
    }
    if let Some(parent) = dir.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io_err("creating", parent, err))?;
    }
    if !dir.exists() {
        std::fs::create_dir(dir).map_err(|err| io_err("creating", dir, err))?;
    }
    Ok(())
}

fn write_sidecar(path: &Path, device: &str) -> ApiResult<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|err| io_err("creating", parent, err))?;
    }
    std::fs::write(path, format!("{device}\n")).map_err(|err| io_err("writing", path, err))
}

fn read_sidecar(path: &Path) -> ApiResult<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => {
            let device = text.trim();
            if device.is_empty() {
                Ok(None)
            } else if loop_device_ok(device) {
                Ok(Some(device.to_owned()))
            } else {
                Err(ApiError::unavailable(format!(
                    "loop device {device} cannot be detached"
                )))
            }
        }
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(io_err("reading", path, err)),
    }
}

fn loop_device_ok(path: &str) -> bool {
    let Some(rest) = path.strip_prefix("/dev/loop") else {
        return false;
    };
    !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_digit()) && path.len() < 64
}

fn command_path_ok(path: &str) -> bool {
    let p = Path::new(path);
    !path.is_empty()
        && path.len() <= 4096
        && !path.contains('\0')
        && !path.chars().any(|c| c.is_whitespace() || c.is_control())
        && p.is_absolute()
        && !p.components().any(|c| matches!(c, Component::ParentDir))
}

fn is_block_device(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::FileTypeExt;
        std::fs::metadata(path)
            .map(|meta| meta.file_type().is_block_device())
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        false
    }
}

fn is_regular_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

fn io_err(action: &str, path: &Path, err: std::io::Error) -> ApiError {
    ApiError::unavailable(format!("{action} {}: {err}", path.display()))
}

#[cfg(test)]
pub(crate) fn env_lock() -> &'static tokio::sync::Mutex<()> {
    static LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
    &LOCK
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::kind;

    struct Tools {
        log: PathBuf,
        empty: PathBuf,
        mkfs_fail: PathBuf,
        mount_fail: PathBuf,
        not_mounted: PathBuf,
        losetup: PathBuf,
        blkid: PathBuf,
        mkfs: PathBuf,
        mount: PathBuf,
        umount: PathBuf,
        sudo: PathBuf,
        sudo_log: PathBuf,
    }

    struct EnvGuard {
        saved: Vec<(&'static str, Option<std::ffi::OsString>)>,
    }

    impl EnvGuard {
        fn set(pairs: &[(&'static str, &Path)]) -> Self {
            let saved = pairs
                .iter()
                .map(|(key, _)| (*key, std::env::var_os(key)))
                .collect();
            for (key, value) in pairs {
                std::env::set_var(key, value);
            }
            Self { saved }
        }
    }

    impl Drop for EnvGuard {
        fn drop(&mut self) {
            for (key, value) in &self.saved {
                match value {
                    Some(value) => std::env::set_var(key, value),
                    None => std::env::remove_var(key),
                }
            }
        }
    }

    fn install(dir: &Path) -> Tools {
        let log = dir.join("tools.log");
        let empty = dir.join("blkid-empty");
        let mkfs_fail = dir.join("mkfs-fail");
        let mount_fail = dir.join("mount-fail");
        let not_mounted = dir.join("umount-not-mounted");
        let sudo_log = dir.join("sudo.log");
        let losetup = dir.join("losetup");
        let blkid = dir.join("blkid");
        let mkfs = dir.join("mkfs.ext4");
        let mount = dir.join("mount");
        let umount = dir.join("umount");
        let sudo = dir.join("sudo");
        write_exe(
            &losetup,
            &tool_body(
                &log,
                "if [ \"$1\" = \"--find\" ]; then printf '%s\\n' /dev/loop7; exit 0; fi\nexit 0\n",
            ),
        );
        write_exe(
            &blkid,
            &tool_body(
                &log,
                &format!(
                    "if [ -f {} ]; then exit 2; fi\nprintf '%s\\n' ext4\nexit 0\n",
                    shell_quote(&empty)
                ),
            ),
        );
        write_exe(
            &mkfs,
            &tool_body(
                &log,
                &format!(
                    "if [ -f {} ]; then echo 'mkfs failed: device busy' >&2; exit 1; fi\nexit 0\n",
                    shell_quote(&mkfs_fail)
                ),
            ),
        );
        write_exe(
            &mount,
            &tool_body(
                &log,
                &format!(
                    "if [ -f {} ]; then echo 'mount failed: no such device' >&2; exit 1; fi\nexit 0\n",
                    shell_quote(&mount_fail)
                ),
            ),
        );
        write_exe(
            &umount,
            &tool_body(
                &log,
                &format!(
                    "if [ -f {} ]; then echo 'umount: not mounted' >&2; exit 32; fi\nexit 0\n",
                    shell_quote(&not_mounted)
                ),
            ),
        );
        write_exe(
            &sudo,
            &format!(
                "#!/bin/sh\nprintf '%s\\n' \"$(basename \"$0\")\" \"$@\" >> {}\nprintf '\\n' >> {}\nif [ \"$1\" = \"-n\" ]; then shift; fi\nexec \"$@\"\n",
                shell_quote(&sudo_log),
                shell_quote(&sudo_log)
            ),
        );
        Tools {
            log,
            empty,
            mkfs_fail,
            mount_fail,
            not_mounted,
            losetup,
            blkid,
            mkfs,
            mount,
            umount,
            sudo,
            sudo_log,
        }
    }

    fn tool_body(log: &Path, behavior: &str) -> String {
        format!(
            "#!/bin/sh\nbase=$(basename \"$0\")\nprintf '%s\\n' \"$base\" \"$@\" >> {}\nprintf '\\n' >> {}\n{behavior}",
            shell_quote(log),
            shell_quote(log)
        )
    }

    fn write_exe(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }

    fn shell_quote(path: &Path) -> String {
        format!("'{}'", path.display().to_string().replace('\'', "'\\''"))
    }

    fn apply_tools(tools: &Tools, data: &Path) -> EnvGuard {
        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
        std::env::remove_var(SUDO_ENV);
        EnvGuard::set(&[
            (LOSETUP_ENV, &tools.losetup),
            (BLKID_ENV, &tools.blkid),
            (MKFS_ENV, &tools.mkfs),
            (MOUNT_ENV, &tools.mount),
            (UMOUNT_ENV, &tools.umount),
            (DATA_ENV, data),
        ])
    }

    struct EuidGuard(Option<std::ffi::OsString>);

    impl EuidGuard {
        fn root() -> Self {
            let saved = std::env::var_os("PISTACK_VOLFS_TEST_EUID");
            std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
            std::env::remove_var(SUDO_ENV);
            Self(saved)
        }
    }

    impl Drop for EuidGuard {
        fn drop(&mut self) {
            match &self.0 {
                Some(value) => std::env::set_var("PISTACK_VOLFS_TEST_EUID", value),
                None => std::env::remove_var("PISTACK_VOLFS_TEST_EUID"),
            }
        }
    }

    fn invocations(text: &str) -> Vec<Vec<String>> {
        let mut out = Vec::new();
        let mut cur = Vec::new();
        for line in text.lines() {
            if line.is_empty() {
                if !cur.is_empty() {
                    out.push(std::mem::take(&mut cur));
                }
            } else {
                cur.push(line.to_owned());
            }
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    }

    fn attach(path: &str, fs: &str, read_only: bool, rbd: bool) -> VolumeAttach {
        VolumeAttach {
            machine: Id::new(kind::MACHINE),
            volume: Id::new(kind::VOLUME),
            path: path.to_owned(),
            device: Some("vdb".into()),
            read_only,
            target: rbd.then(|| DiskTarget::Rbd {
                pool: "volumes".into(),
                image: "img-1".into(),
                monitors: Vec::new(),
                user: "pistack".into(),
                key_secret: None,
            }),
            fs_type: fs.to_owned(),
        }
    }

    fn file_plan<'a>(
        source: &'a str,
        loop_device: &'a str,
        mount_dir: &'a str,
        fs: &'a str,
        has_filesystem: bool,
        read_only: bool,
    ) -> MountPlan<'a> {
        MountPlan {
            kind: DiskKind::File,
            source,
            loop_device,
            mount_dir,
            fs_type: fs,
            has_filesystem,
            read_only,
        }
    }

    #[test]
    fn command_plan_for_each_backend() {
        let local = attach_plan(&file_plan(
            "/var/lib/pistack/volumes/disk.raw",
            "/dev/loop7",
            "/var/lib/pistack/slab/mounts/vol-1",
            "ext4",
            false,
            false,
        ))
        .unwrap();
        assert_eq!(
            local,
            vec![
                vec![
                    "losetup",
                    "--find",
                    "--show",
                    "/var/lib/pistack/volumes/disk.raw"
                ],
                vec!["blkid", "-p", "-o", "value", "-s", "TYPE", "/dev/loop7"],
                vec!["mkfs.ext4", "-q", "-F", "/dev/loop7"],
                vec!["mount", "/dev/loop7", "/var/lib/pistack/slab/mounts/vol-1"],
            ]
            .into_iter()
            .map(|step| step.into_iter().map(str::to_owned).collect())
            .collect::<Vec<Vec<String>>>()
        );

        let lvm = attach_plan(&MountPlan {
            kind: DiskKind::Device,
            source: "/dev/vg0/vol-1",
            loop_device: "",
            mount_dir: "/var/lib/pistack/slab/mounts/vol-1",
            fs_type: "ext4",
            has_filesystem: false,
            read_only: false,
        })
        .unwrap();
        assert_eq!(lvm[0][0], "blkid");
        assert_eq!(lvm[0].last().map(String::as_str), Some("/dev/vg0/vol-1"));
        assert!(lvm.iter().all(|step| step[0] != "losetup"));
        assert_eq!(
            lvm.iter().find(|step| step[0] == "mkfs.ext4").unwrap()[1..],
            ["-q", "-F", "/dev/vg0/vol-1"]
        );
        assert_eq!(lvm.last().unwrap()[0], "mount");

        let rbd = attach_plan(&MountPlan {
            kind: DiskKind::Device,
            source: "/dev/rbd0",
            loop_device: "",
            mount_dir: "/var/lib/pistack/slab/mounts/vol-1",
            fs_type: "",
            has_filesystem: false,
            read_only: false,
        })
        .unwrap();
        assert_eq!(rbd[0].last().map(String::as_str), Some("/dev/rbd0"));
        assert_eq!(rbd[1][0], "mkfs.ext4");
        assert!(rbd.iter().all(|step| step[0] != "losetup"));

        let xfs = attach_plan(&MountPlan {
            kind: DiskKind::Device,
            source: "/dev/vg0/vol-1",
            loop_device: "",
            mount_dir: "/mnt/vol",
            fs_type: "xfs",
            has_filesystem: false,
            read_only: false,
        })
        .unwrap();
        assert_eq!(
            xfs.iter().find(|step| step[0] == "mkfs.xfs").unwrap()[1..],
            ["-f", "-q", "/dev/vg0/vol-1"]
        );
    }

    #[test]
    fn existing_filesystem_is_not_in_the_plan() {
        let plan = attach_plan(&file_plan(
            "/var/lib/pistack/volumes/disk.raw",
            "/dev/loop7",
            "/mnt/vol",
            "ext4",
            true,
            false,
        ))
        .unwrap();
        assert!(plan.iter().all(|step| !step[0].starts_with("mkfs")));
        assert_eq!(plan.len(), 3);

        let ro = attach_plan(&file_plan(
            "/var/lib/pistack/volumes/disk.raw",
            "/dev/loop7",
            "/mnt/vol",
            "ext4",
            true,
            true,
        ))
        .unwrap();
        assert_eq!(
            ro.last().unwrap()[1..],
            ["-o", "ro", "/dev/loop7", "/mnt/vol"]
        );
    }

    #[test]
    fn detach_plan_unmounts_then_detaches_a_loop() {
        let with_loop = detach_plan("/mnt/vol", Some("/dev/loop7"));
        assert_eq!(
            with_loop,
            vec![
                vec!["umount".to_owned(), "/mnt/vol".to_owned()],
                vec![
                    "losetup".to_owned(),
                    "-d".to_owned(),
                    "/dev/loop7".to_owned()
                ],
            ]
        );
        let device = detach_plan("/mnt/vol", None);
        assert_eq!(
            device,
            vec![vec!["umount".to_owned(), "/mnt/vol".to_owned()]]
        );
    }

    #[test]
    fn filesystem_type_rejects_a_command_string() {
        assert_eq!(canonical_fs("").unwrap(), "ext4");
        assert_eq!(canonical_fs("  XFS ").unwrap(), "xfs");
        assert!(canonical_fs("ext4;rm").is_err());
        assert!(canonical_fs("../ext4").is_err());
        assert!(canonical_fs("e").is_err());
    }

    #[tokio::test]
    async fn local_file_formats_and_records_the_mount_directory() {
        let _lock = env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install(tmp.path());
        let data = tmp.path().join("data");
        let _env = apply_tools(&tools, &data);
        let _euid = EuidGuard::root();
        std::fs::write(&tools.empty, b"1").unwrap();
        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"raw").unwrap();
        let file = file.to_str().unwrap();
        let req = attach(file, "ext4", false, false);
        let mounted = mount_for_pod(&req, file).await.unwrap();
        let (dir, _) = volume_paths(&req.volume).unwrap();
        assert_eq!(mounted, dir.to_string_lossy());
        assert!(dir.is_dir());
        let plan = attach_plan(&file_plan(
            file,
            "/dev/loop7",
            dir.to_str().unwrap(),
            "ext4",
            false,
            false,
        ))
        .unwrap();
        assert_eq!(
            invocations(&std::fs::read_to_string(&tools.log).unwrap()),
            plan
        );
    }

    #[tokio::test]
    async fn blkid_type_skips_mkfs_for_file_lvm_and_rbd() {
        let _lock = env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install(tmp.path());
        let data = tmp.path().join("data");
        let _env = apply_tools(&tools, &data);
        let _euid = EuidGuard::root();
        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"raw").unwrap();
        let file = file.to_string_lossy().into_owned();
        let req = attach(&file, "", false, false);
        mount_for_pod(&req, &file).await.unwrap();
        let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
        assert!(
            log.iter().all(|step| !step[0].starts_with("mkfs")),
            "{log:?}"
        );
        release_volume(&req.volume).await.unwrap();

        let _ = std::fs::write(&tools.log, b"");
        let lvm = attach("/dev/vg0/vol-1", "ext4", false, false);
        mount_for_pod(&lvm, "/dev/vg0/vol-1").await.unwrap();
        let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
        assert_eq!(log[0][0], "blkid");
        assert_eq!(log[0].last().map(String::as_str), Some("/dev/vg0/vol-1"));
        assert!(log.iter().all(|step| step[0] != "losetup"));
        assert!(log.iter().all(|step| !step[0].starts_with("mkfs")));
        release_volume(&lvm.volume).await.unwrap();

        let _ = std::fs::write(&tools.log, b"");
        let rbd = attach("", "ext4", false, true);
        let mounted = mount_for_pod(&rbd, "/dev/rbd0").await.unwrap();
        let (dir, _) = volume_paths(&rbd.volume).unwrap();
        assert_eq!(mounted, dir.to_string_lossy());
        let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
        assert_eq!(log[0].last().map(String::as_str), Some("/dev/rbd0"));
        assert!(log
            .iter()
            .all(|step| step[0] != "losetup" && !step[0].starts_with("mkfs")));
    }

    #[tokio::test]
    async fn mount_failure_rolls_back_the_loop_device() {
        let _lock = env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install(tmp.path());
        let data = tmp.path().join("data");
        let _env = apply_tools(&tools, &data);
        let _euid = EuidGuard::root();
        std::fs::write(&tools.empty, b"1").unwrap();
        std::fs::write(&tools.mount_fail, b"1").unwrap();
        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"raw").unwrap();
        let file = file.to_string_lossy().into_owned();
        let req = attach(&file, "ext4", false, false);
        let err = mount_for_pod(&req, &file).await.unwrap_err();
        assert!(err.message.contains("mount"), "{err}");
        let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
        let names: Vec<&str> = log.iter().map(|step| step[0].as_str()).collect();
        assert_eq!(
            names,
            [
                "losetup",
                "blkid",
                "mkfs.ext4",
                "mount",
                "umount",
                "losetup"
            ]
        );
        assert_eq!(log.last().unwrap()[1..], ["-d", "/dev/loop7"]);
        let (dir, sidecar) = volume_paths(&req.volume).unwrap();
        assert!(!dir.exists());
        assert!(!sidecar.exists());
    }

    #[tokio::test]
    async fn mkfs_failure_does_not_mount_and_detaches_the_loop() {
        let _lock = env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install(tmp.path());
        let data = tmp.path().join("data");
        let _env = apply_tools(&tools, &data);
        let _euid = EuidGuard::root();
        std::fs::write(&tools.empty, b"1").unwrap();
        std::fs::write(&tools.mkfs_fail, b"1").unwrap();
        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"raw").unwrap();
        let file = file.to_string_lossy().into_owned();
        let req = attach(&file, "ext4", false, false);
        let err = mount_for_pod(&req, &file).await.unwrap_err();
        assert!(err.message.contains("mkfs"), "{err}");
        let names: Vec<String> = invocations(&std::fs::read_to_string(&tools.log).unwrap())
            .into_iter()
            .map(|step| step[0].clone())
            .collect();
        assert_eq!(
            names,
            ["losetup", "blkid", "mkfs.ext4", "losetup"]
                .map(str::to_owned)
                .to_vec()
        );
    }

    #[tokio::test]
    async fn detach_is_ordered_and_idempotent_when_already_unmounted() {
        let _lock = env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install(tmp.path());
        let data = tmp.path().join("data");
        let _env = apply_tools(&tools, &data);
        let _euid = EuidGuard::root();
        std::fs::write(&tools.empty, b"1").unwrap();
        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"raw").unwrap();
        let file = file.to_string_lossy().into_owned();
        let req = attach(&file, "ext4", false, false);
        mount_for_pod(&req, &file).await.unwrap();
        let _ = std::fs::write(&tools.log, b"");
        release_volume(&req.volume).await.unwrap();
        let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
        let (dir, _) = volume_paths(&req.volume).unwrap();
        assert_eq!(log, detach_plan(dir.to_str().unwrap(), Some("/dev/loop7")));
        let before = std::fs::read_to_string(&tools.log).unwrap();
        release_volume(&req.volume).await.unwrap();
        assert_eq!(std::fs::read_to_string(&tools.log).unwrap(), before);

        let (dir, sidecar) = volume_paths(&req.volume).unwrap();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(&sidecar, "/dev/loop7\n").unwrap();
        std::fs::write(&tools.not_mounted, b"1").unwrap();
        let _ = std::fs::write(&tools.log, b"");
        release_volume(&req.volume).await.unwrap();
        let log = invocations(&std::fs::read_to_string(&tools.log).unwrap());
        assert_eq!(log[0][0], "umount");
        assert_eq!(log[1][1..], ["-d", "/dev/loop7"]);
        assert!(!sidecar.exists());
    }

    #[tokio::test]
    async fn non_root_agent_runs_the_tools_through_sudo() {
        let _lock = env_lock().lock().await;
        let tmp = tempfile::tempdir().unwrap();
        let tools = install(tmp.path());
        let data = tmp.path().join("data");
        let _env = apply_tools(&tools, &data);
        let saved_euid = std::env::var_os("PISTACK_VOLFS_TEST_EUID");
        let saved_sudo = std::env::var_os(SUDO_ENV);
        let saved_sudo_bin = std::env::var_os(SUDO_BIN_ENV);
        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "1000");
        std::env::set_var(SUDO_ENV, "1");
        std::env::set_var(SUDO_BIN_ENV, &tools.sudo);
        std::fs::write(&tools.empty, b"1").unwrap();
        let file = tmp.path().join("disk.raw");
        std::fs::write(&file, b"raw").unwrap();
        let file = file.to_string_lossy().into_owned();
        let req = attach(&file, "ext4", false, false);
        let mounted = mount_for_pod(&req, &file).await.unwrap();
        assert!(mounted.contains("/slab/mounts/"));
        let sudo_log = invocations(&std::fs::read_to_string(&tools.sudo_log).unwrap());
        let elevated: Vec<&str> = sudo_log
            .iter()
            .filter_map(|step| step.get(2).map(String::as_str))
            .collect();
        assert!(sudo_log
            .iter()
            .all(|step| step.get(1).map(String::as_str) == Some("-n")));
        assert!(elevated.iter().any(|path| path.ends_with("losetup")));
        assert!(elevated.iter().any(|path| path.ends_with("blkid")));
        assert!(elevated.iter().any(|path| path.ends_with("mkfs.ext4")));
        assert!(elevated.iter().any(|path| path.ends_with("mount")));
        release_volume(&req.volume).await.unwrap();
        let sudo_log = invocations(&std::fs::read_to_string(&tools.sudo_log).unwrap());
        assert!(sudo_log.iter().any(|step| {
            step.get(2).is_some_and(|path| path.ends_with("umount"))
                && step.get(1).map(String::as_str) == Some("-n")
        }));

        std::env::set_var("PISTACK_VOLFS_TEST_EUID", "0");
        let _ = std::fs::write(&tools.sudo_log, b"");
        let _ = std::fs::write(&tools.log, b"");
        mount_for_pod(&req, &file).await.unwrap();
        assert_eq!(std::fs::read_to_string(&tools.sudo_log).unwrap(), "");
        assert!(!std::fs::read_to_string(&tools.log).unwrap().is_empty());

        match saved_euid {
            Some(value) => std::env::set_var("PISTACK_VOLFS_TEST_EUID", value),
            None => std::env::remove_var("PISTACK_VOLFS_TEST_EUID"),
        }
        match saved_sudo {
            Some(value) => std::env::set_var(SUDO_ENV, value),
            None => std::env::remove_var(SUDO_ENV),
        }
        match saved_sudo_bin {
            Some(value) => std::env::set_var(SUDO_BIN_ENV, value),
            None => std::env::remove_var(SUDO_BIN_ENV),
        }
    }
}
