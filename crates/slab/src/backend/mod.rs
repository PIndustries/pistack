// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Storage backends. `slab` decides *what* exists; a `BlockBackend` makes it
//! exist on real storage and tells `anvil` how to reach it.
//!
//! The volume type selects the backend. Empty or `local` is the sparse file
//! backend and stays the default. `lvm` runs only when the volume group, thin
//! pool, and LVM tools exist. `rbd` runs only when the pool, monitors, and
//! `rbd` binary exist. A missing binary or pool is unavailable.
//!
//! A local file and an LVM volume exist on one node, the one that ran the
//! backend. The client records it as `status.home_node` and refuses to
//! attach the volume anywhere else. Only `rbd` is reachable from every host.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::types::{Snapshot, Volume};

/// `$PISTACK_QEMU_IMG_BIN`, when set and non-empty, replaces `qemu-img`.
pub(crate) const ENV_QEMU_IMG: &str = "PISTACK_QEMU_IMG_BIN";
const QEMU_IMG_TIMEOUT: Duration = Duration::from_secs(60 * 60);

pub mod local;
pub mod lvm;
pub mod rbd;

pub use local::LocalFileBackend;
pub use lvm::LvmBackend;
pub use rbd::RbdBackend;

/// Opaque handle a backend returns for something it created. Stored in
/// `status.backend_ref` so the backend can find it again.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct BackendRef(pub String);

impl BackendRef {
    pub fn new(s: impl Into<String>) -> Self {
        Self(s.into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl From<BackendRef> for String {
    fn from(r: BackendRef) -> Self {
        r.0
    }
}

impl std::fmt::Display for BackendRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// Transport a host uses to reach a volume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Protocol {
    /// A path on the host itself (file or LV).
    Local,
    Iscsi,
    Rbd,
    Nvme,
}

/// Protocol-specific details `anvil` needs to attach.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "protocol", rename_all = "snake_case")]
pub enum AttachTarget {
    Local {
        path: String,
    },
    Iscsi {
        portal: String,
        iqn: String,
        lun: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        auth_secret: Option<String>,
    },
    Rbd {
        pool: String,
        image: String,
        monitors: Vec<String>,
        user: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        key_secret: Option<String>,
    },
    Nvme {
        transport: String,
        address: String,
        port: u16,
        nqn: String,
    },
}

impl AttachTarget {
    pub fn protocol(&self) -> Protocol {
        match self {
            AttachTarget::Local { .. } => Protocol::Local,
            AttachTarget::Iscsi { .. } => Protocol::Iscsi,
            AttachTarget::Rbd { .. } => Protocol::Rbd,
            AttachTarget::Nvme { .. } => Protocol::Nvme,
        }
    }
}

/// Everything `anvil` needs to attach a volume on a given host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachInfo {
    pub protocol: Protocol,
    pub target: AttachTarget,
    /// Guest device name the user asked for (`vdb`), if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device: Option<String>,
    pub read_only: bool,
}

impl AttachInfo {
    pub fn new(target: AttachTarget) -> Self {
        Self {
            protocol: target.protocol(),
            target,
            device: None,
            read_only: false,
        }
    }
}

/// The seam between `slab` and real storage. All methods take the resource
/// so backends can read `spec` and `status.backend_ref`.
#[async_trait]
pub trait BlockBackend: Send + Sync + 'static {
    /// Short name matching `VolumeTypeSpec::backend`.
    fn name(&self) -> &'static str;

    /// Create blank storage for `vol` at `spec.size_gib`. Image bytes are
    /// written afterwards by [`BlockBackend::write_image`]. A volume whose
    /// source is a snapshot or another volume is made by
    /// [`BlockBackend::restore`] or [`BlockBackend::clone_volume`].
    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef>;

    /// Copy the disk image at `src` onto `vol` as raw bytes. `format` is
    /// `raw` or `qcow2` and is passed to `qemu-img` as `-f` so the tool
    /// does not probe. The volume from [`BlockBackend::create`] must
    /// already exist at its full size (`qemu-img -n`).
    async fn write_image(&self, vol: &Volume, src: &Path, format: &str) -> ApiResult<()>;

    /// Delete the storage behind `vol`. Must be idempotent.
    async fn delete(&self, vol: &Volume) -> ApiResult<()>;

    /// Take a point-in-time snapshot of `vol`.
    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef>;

    /// Delete a snapshot. Must be idempotent.
    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()>;

    /// Materialise `snap` as the new volume `into`, at `into.spec.size_gib`.
    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef>;

    /// Copy the volume `src` into the new volume `into`, at
    /// `into.spec.size_gib`.
    async fn clone_volume(&self, src: &Volume, into: &Volume) -> ApiResult<BackendRef>;

    /// Grow `vol` to `new_size_gib`. Shrinking is never allowed.
    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()>;

    /// How `host` should attach `vol`.
    async fn attach_info(&self, vol: &Volume, host: &str) -> ApiResult<AttachInfo>;

    /// File that holds the volume bytes for a backup, when the backend has
    /// one inside its own directory. LVM and RBD do not. Streaming backup
    /// uses [`BlockBackend::open_backup`].
    async fn bytes_path(&self, vol: &Volume) -> ApiResult<std::path::PathBuf>;

    /// Open `vol` for a streaming backup read. Local files and LVM devices
    /// are opened directly. RBD runs `rbd export <pool>/<id> -`.
    async fn open_backup(&self, vol: &Volume) -> ApiResult<BackupStream>;

    /// Open `vol` for a streaming restore write, positioned at byte 0.
    async fn open_restore(&self, vol: &Volume) -> ApiResult<RestoreStream>;
}

/// Read handle for one backup. Dropping it stops an in-progress export.
pub struct BackupStream {
    kind: ReadKind,
}

impl std::fmt::Debug for BackupStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            ReadKind::File(_) => f.write_str("BackupStream::File"),
            ReadKind::Child { label, .. } => write!(f, "BackupStream::Child({label})"),
        }
    }
}

enum ReadKind {
    File(tokio::fs::File),
    Child {
        stdout: tokio::process::ChildStdout,
        child: tokio::process::Child,
        stderr: tokio::sync::oneshot::Receiver<String>,
        label: &'static str,
    },
}

impl BackupStream {
    pub(crate) fn file(file: tokio::fs::File) -> Self {
        Self {
            kind: ReadKind::File(file),
        }
    }

    /// Spawn `bin args…` and read its stdout. Stderr is drained so a noisy
    /// command cannot fill the pipe and stall the export.
    pub(crate) fn spawn(bin: &str, args: &[String], label: &'static str) -> ApiResult<Self> {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::unavailable(format!("{label} is not available"))
            } else {
                ApiError::internal(format!("{label}: {e}"))
            }
        })?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| ApiError::internal(format!("{label} has no stdout")))?;
        let stderr = child.stderr.take();
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Some(stderr) = stderr {
            tokio::spawn(async move {
                let detail = drain_stderr(stderr).await;
                let _ = tx.send(detail);
            });
        } else {
            let _ = tx.send(String::new());
        }
        Ok(Self {
            kind: ReadKind::Child {
                stdout,
                child,
                stderr: rx,
                label,
            },
        })
    }

    pub async fn read(&mut self, buf: &mut [u8]) -> ApiResult<usize> {
        let result = match &mut self.kind {
            ReadKind::File(file) => file.read(buf).await,
            ReadKind::Child { stdout, .. } => stdout.read(buf).await,
        };
        result.map_err(|e| ApiError::internal(format!("backup read: {e}")))
    }

    /// Wait for an export command to exit. A file has nothing to wait for.
    pub async fn finish(self) -> ApiResult<()> {
        match self.kind {
            ReadKind::File(_) => Ok(()),
            ReadKind::Child {
                stdout,
                mut child,
                stderr,
                label,
            } => {
                drop(stdout);
                let status = child
                    .wait()
                    .await
                    .map_err(|e| ApiError::internal(format!("{label}: {e}")))?;
                let detail = stderr.await.unwrap_or_default();
                if status.success() {
                    return Ok(());
                }
                let code = status.code().unwrap_or(-1);
                let detail = if detail.is_empty() {
                    format!("exit {code}")
                } else {
                    format!("exit {code}: {detail}")
                };
                Err(ApiError::internal(format!("{label} failed: {detail}")))
            }
        }
    }
}

async fn drain_stderr(mut stderr: tokio::process::ChildStderr) -> String {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 512];
    loop {
        match stderr.read(&mut tmp).await {
            Ok(0) | Err(_) => break,
            Ok(n) => {
                buf.extend_from_slice(&tmp[..n]);
                if buf.len() > 400 {
                    buf.drain(..buf.len() - 400);
                }
            }
        }
    }
    String::from_utf8_lossy(&buf).trim().to_owned()
}

/// Work a backend still has to do once every byte of a restore has been
/// written. It is given the number of bytes written.
pub(crate) type RestoreFinish = Box<
    dyn FnOnce(u64) -> std::pin::Pin<Box<dyn std::future::Future<Output = ApiResult<()>> + Send>>
        + Send,
>;

/// Write handle for a restore. The caller writes the object from byte 0.
/// Dropping it before [`RestoreStream::finish`] stops an import command.
pub struct RestoreStream {
    kind: WriteKind,
    written: u64,
}

impl std::fmt::Debug for RestoreStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match &self.kind {
            WriteKind::File(_) => f.write_str("RestoreStream::File"),
            WriteKind::Child { label, .. } => write!(f, "RestoreStream::Child({label})"),
        }
    }
}

enum WriteKind {
    File(tokio::fs::File),
    Child {
        stdin: tokio::process::ChildStdin,
        child: tokio::process::Child,
        stderr: tokio::sync::oneshot::Receiver<String>,
        label: &'static str,
        after: RestoreFinish,
    },
}

impl RestoreStream {
    pub(crate) fn file(file: tokio::fs::File) -> Self {
        Self {
            kind: WriteKind::File(file),
            written: 0,
        }
    }

    /// Spawn `bin args…` and write the restore to its stdin. `after` runs
    /// once the command has exited with success.
    pub(crate) fn spawn(
        bin: &str,
        args: &[String],
        label: &'static str,
        after: RestoreFinish,
    ) -> ApiResult<Self> {
        let mut cmd = tokio::process::Command::new(bin);
        cmd.args(args)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        let mut child = cmd.spawn().map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                ApiError::unavailable(format!("{label} is not available"))
            } else {
                ApiError::internal(format!("{label}: {e}"))
            }
        })?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| ApiError::internal(format!("{label} has no stdin")))?;
        let (tx, rx) = tokio::sync::oneshot::channel();
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let detail = drain_stderr(stderr).await;
                let _ = tx.send(detail);
            });
        } else {
            let _ = tx.send(String::new());
        }
        Ok(Self {
            kind: WriteKind::Child {
                stdin,
                child,
                stderr: rx,
                label,
                after,
            },
            written: 0,
        })
    }

    pub async fn write_all(&mut self, buf: &[u8]) -> ApiResult<()> {
        let result = match &mut self.kind {
            WriteKind::File(file) => file.write_all(buf).await,
            WriteKind::Child { stdin, .. } => stdin.write_all(buf).await,
        };
        result.map_err(|e| ApiError::internal(format!("backup restore: {e}")))?;
        self.written += buf.len() as u64;
        Ok(())
    }

    pub async fn finish(self) -> ApiResult<()> {
        let written = self.written;
        match self.kind {
            WriteKind::File(mut file) => {
                file.flush()
                    .await
                    .map_err(|e| ApiError::internal(format!("backup restore: {e}")))?;
                file.sync_all()
                    .await
                    .map_err(|e| ApiError::internal(format!("backup restore: {e}")))?;
                Ok(())
            }
            WriteKind::Child {
                mut stdin,
                mut child,
                stderr,
                label,
                after,
            } => {
                // The command reads to end of input, so close it first.
                let flushed = stdin.shutdown().await;
                drop(stdin);
                let status = child
                    .wait()
                    .await
                    .map_err(|e| ApiError::internal(format!("{label}: {e}")))?;
                let detail = stderr.await.unwrap_or_default();
                if !status.success() {
                    let code = status.code().unwrap_or(-1);
                    let detail = if detail.is_empty() {
                        format!("exit {code}")
                    } else {
                        format!("exit {code}: {detail}")
                    };
                    return Err(ApiError::internal(format!("{label} failed: {detail}")));
                }
                flushed.map_err(|e| ApiError::internal(format!("{label}: {e}")))?;
                after(written).await
            }
        }
    }
}

pub(crate) fn gib_to_bytes(gib: u64) -> u64 {
    gib.saturating_mul(1024 * 1024 * 1024)
}

/// `$PISTACK_*_BIN` when set and non-empty, otherwise `default`.
pub(crate) fn configured_bin(env_key: &str, default: &str) -> String {
    match std::env::var(env_key) {
        Ok(v) => {
            let v = v.trim();
            if v.is_empty() {
                default.to_owned()
            } else {
                v.to_owned()
            }
        }
        Err(_) => default.to_owned(),
    }
}

/// The configured binary, or unavailable when it is not executable.
pub(crate) fn configured_bin_ready(env_key: &str, default: &str) -> ApiResult<String> {
    let bin = configured_bin(env_key, default);
    if command_exists(&bin) {
        Ok(bin)
    } else {
        Err(ApiError::unavailable(format!("{default} is not available")))
    }
}

fn command_exists(bin: &str) -> bool {
    let path = std::path::Path::new(bin);
    if path.is_absolute() || bin.contains('/') || bin.contains('\\') {
        return is_executable(path);
    }
    match std::env::var_os("PATH") {
        Some(paths) => std::env::split_paths(&paths).any(|dir| is_executable(&dir.join(bin))),
        None => false,
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// `qemu-img convert -O raw -f <format> -n <src> <dest>`.
///
/// `-f` stops qemu from probing (a probed raw file can name a backing
/// file). `-n` writes into the volume created just before this instead of
/// replacing it, which is what keeps a pre-sized file, LV, or rbd image
/// at `size_gib`.
pub(crate) fn qemu_img_convert_argv(format: &str, src: &Path, dest: &str) -> Vec<String> {
    let mut args = Vec::with_capacity(8);
    args.push("convert".to_owned());
    args.push("-O".to_owned());
    args.push("raw".to_owned());
    if !format.is_empty() {
        args.push("-f".to_owned());
        args.push(format.to_owned());
    }
    args.push("-n".to_owned());
    args.push(src.display().to_string());
    args.push(dest.to_owned());
    args
}

/// Run `args` with `qemu-img` (or [`ENV_QEMU_IMG`]). `src` must be a file.
pub(crate) async fn run_qemu_img(src: &Path, args: &[String]) -> ApiResult<()> {
    if !src.is_file() {
        return Err(ApiError::not_found(format!(
            "image file {} does not exist",
            src.display()
        )));
    }
    let bin = configured_bin(ENV_QEMU_IMG, "qemu-img");
    let mut cmd = tokio::process::Command::new(&bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let finished = tokio::time::timeout(QEMU_IMG_TIMEOUT, cmd.output()).await;
    let output = match finished {
        Err(_) => {
            return Err(ApiError::unavailable(format!(
                "qemu-img timed out after {}s",
                QEMU_IMG_TIMEOUT.as_secs()
            )))
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::unavailable("qemu-img is not available"))
        }
        Ok(Err(e)) => return Err(ApiError::internal(format!("qemu-img: {e}"))),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let code = output.status.code().unwrap_or(-1);
    let detail = format!("{}{}", clip(&output.stderr), clip(&output.stdout));
    let detail = if detail.is_empty() {
        format!("exit {code}")
    } else {
        detail
    };
    Err(ApiError::internal(format!(
        "qemu-img convert failed: {detail}"
    )))
}

fn clip(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim()
        .chars()
        .take(400)
        .collect()
}

#[cfg(test)]
pub(crate) static QEMU_IMG_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
