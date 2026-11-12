// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Ceph RBD backend.
//!
//! `$PISTACK_RBD_BIN`, when set and non-empty, replaces the `rbd` binary.
//! Otherwise `rbd` is looked up on `PATH`. A missing binary is unavailable.
//! Pool, volume ids and snapshot ids must match `[A-Za-z0-9._-]{1,64}`.
//!
//! | operation       | command |
//! | --------------- | ------- |
//! | create          | `rbd create --size {size_gib}G --image-feature layering,exclusive-lock {pool}/{vol_id}` |
//! | delete          | `rbd rm {pool}/{vol_id}` |
//! | snapshot        | `rbd snap create {pool}/{vol_id}@{snap_id}` then `rbd snap protect {pool}/{vol_id}@{snap_id}` |
//! | delete_snapshot | `rbd snap unprotect {pool}/{vol_id}@{snap_id}` then `rbd snap rm {pool}/{vol_id}@{snap_id}` |
//! | restore         | `rbd clone {pool}/{vol_id}@{snap_id} {pool}/{new_vol_id}` and, when the new volume is larger, `rbd resize --size {size_gib}G {pool}/{new_vol_id}` |
//! | clone           | `rbd cp --image-feature layering,exclusive-lock {pool}/{src_vol_id} {pool}/{new_vol_id}`, then `rbd resize` as for restore |
//! | extend          | `rbd resize --size {new_size_gib}G {pool}/{vol_id}` |
//! | attach_info     | `Rbd { pool, image: vol_id, monitors, user }` |
//! | image           | `qemu-img convert -O raw -f <fmt> -n <src> rbd:{pool}/{vol_id}` |
//! | backup          | `rbd export {pool}/{vol_id} -` (stdout is the byte stream) |
//! | backup restore  | `rbd import --image-feature layering,exclusive-lock - {pool}/{vol_id}.restore` (stdin is the byte stream), then `rbd rm {pool}/{vol_id}`, `rbd rename {pool}/{vol_id}.restore {pool}/{vol_id}` and, when the stream was shorter than the volume, `rbd resize` |
//! | pool (opt-in)   | `ceph osd pool create {pool}` then `rbd pool init {pool}` |
//!
//! The image is created with `rbd create` first. `qemu-img convert -n`
//! then writes raw bytes into that image (`-n` is qemu-img's "do not
//! create the target", and its help cites rbd). `rbd import` would store
//! a qcow2 file's bytes unchanged and would not fill the image `rbd
//! create` just made. Monitors stay in the host ceph config, the same way
//! `rbd create` is invoked.
//!
//! `rbd import` cannot write into an image that exists, so a restore
//! imports into `{vol_id}.restore` and swaps it in once the import has
//! exited cleanly. Until then the volume is untouched. A volume with
//! snapshots is refused before any byte is read: removing the image
//! would fail, and the snapshots would not describe the restored bytes.
//!
//! Parameters from `VolumeTypeSpec::params`: `pool` (default `volumes`),
//! `user` (default `pistack`), `monitors` (comma separated), `create_pool`
//! (`true` or `false`, default `false`).
//!
//! A mutating command runs only when monitors are configured, `rbd` is
//! executable, and `rbd pool stats {pool}` succeeds. A missing binary,
//! monitor list, or pool is unavailable. The pool is created only when
//! the volume type sets `create_pool=true`; placement group counts and
//! replication are the operator's call, so the default is to say which
//! commands to run. `$PISTACK_CEPH_BIN` replaces the `ceph` binary.

use async_trait::async_trait;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

use super::{AttachInfo, AttachTarget, BackendRef, BackupStream, BlockBackend, RestoreStream};
use crate::types::{Snapshot, Volume};

const TIMEOUT: Duration = Duration::from_secs(30);
const ENV_BIN: &str = "PISTACK_RBD_BIN";
const ENV_CEPH_BIN: &str = "PISTACK_CEPH_BIN";
const IMAGE_FEATURES: &str = "layering,exclusive-lock";

pub struct RbdBackend {
    pub pool: String,
    pub user: String,
    pub monitors: Vec<String>,
    /// Create the pool when it does not exist.
    pub create_pool: bool,
}

impl Default for RbdBackend {
    fn default() -> Self {
        Self {
            pool: "volumes".into(),
            user: "pistack".into(),
            monitors: Vec::new(),
            create_pool: false,
        }
    }
}

impl RbdBackend {
    pub fn new(pool: impl Into<String>, user: impl Into<String>, monitors: Vec<String>) -> Self {
        Self {
            pool: pool.into(),
            user: user.into(),
            monitors,
            create_pool: false,
        }
    }

    /// Create the pool when it is missing.
    pub fn with_create_pool(mut self, create: bool) -> Self {
        self.create_pool = create;
        self
    }

    /// `pool`, `user`, comma-separated `monitors` and `create_pool` from a
    /// volume type.
    pub fn from_params(params: &BTreeMap<String, String>) -> Self {
        Self {
            pool: params.get("pool").cloned().unwrap_or_default(),
            user: params.get("user").cloned().unwrap_or_default(),
            monitors: split_monitors(params.get("monitors")),
            create_pool: params
                .get("create_pool")
                .and_then(|raw| parse_bool(raw))
                .unwrap_or(false),
        }
    }

    /// Reject parameters no command could use. A parameter that is left
    /// out is not an error here: the type can be created first and
    /// completed later, and stays unavailable until then.
    pub fn validate_params(params: &BTreeMap<String, String>) -> ApiResult<()> {
        if let Some(pool) = params.get("pool").filter(|p| !p.is_empty()) {
            ident("pool", pool)?;
        }
        if let Some(user) = params.get("user").filter(|u| !u.is_empty()) {
            ident("user", user)?;
        }
        for monitor in split_monitors(params.get("monitors")) {
            let ok = monitor.len() <= 255
                && monitor.bytes().all(|b| {
                    b.is_ascii_alphanumeric() || matches!(b, b'.' | b':' | b'-' | b'[' | b']')
                });
            if !ok {
                return Err(ApiError::bad_request(format!(
                    "monitor '{monitor}' is not a host or host:port"
                )));
            }
        }
        if let Some(raw) = params.get("create_pool") {
            if parse_bool(raw).is_none() {
                return Err(ApiError::bad_request(
                    "create_pool must be 'true' or 'false'",
                ));
            }
        }
        Ok(())
    }

    /// Monitors, the `rbd` binary, and the pool must all exist. No `rbd`
    /// create, remove, clone, or resize runs before this returns.
    pub async fn require_available(&self) -> ApiResult<()> {
        let pool = self.pool()?.to_owned();
        if !self.monitors.iter().any(|m| !m.trim().is_empty()) {
            return Err(ApiError::unavailable("rbd monitors are not configured"));
        }
        let bin = super::configured_bin_ready(ENV_BIN, "rbd")?;
        let stats = ["pool".into(), "stats".into(), pool.clone()];
        let probe = match run(&bin, &stats, false).await {
            Ok(()) => return Ok(()),
            Err(e) if e.code == ErrorCode::Unavailable => return Err(e),
            Err(e) => e,
        };
        if !self.create_pool {
            return Err(ApiError::unavailable(format!(
                "rbd pool {pool} is not available; create it with \
                 `ceph osd pool create {pool}` and `rbd pool init {pool}`, \
                 or set create_pool=true on the volume type"
            )));
        }
        tracing::info!(pool = %pool, probe = %probe.message, "creating the rbd pool");
        let ceph = super::configured_bin_ready(ENV_CEPH_BIN, "ceph")?;
        let created = async {
            let create = ["osd".into(), "pool".into(), "create".into(), pool.clone()];
            run(&ceph, &create, false).await?;
            run(&bin, &["pool".into(), "init".into(), pool.clone()], false).await?;
            run(&bin, &stats, false).await
        }
        .await;
        created.map_err(|e| {
            if e.code == ErrorCode::Unavailable {
                e
            } else {
                ApiError::unavailable(format!(
                    "rbd pool {pool} could not be created: {}",
                    e.message
                ))
            }
        })
    }

    fn pool(&self) -> ApiResult<&str> {
        let pool = if self.pool.is_empty() {
            "volumes"
        } else {
            self.pool.as_str()
        };
        ident("pool", pool)?;
        Ok(pool)
    }

    fn user(&self) -> String {
        if self.user.is_empty() {
            "pistack".into()
        } else {
            self.user.clone()
        }
    }

    /// A clone or a copy has its source's size. Grow it when `into` asks
    /// for more than `source_gib`.
    async fn grow(&self, image: &str, source_gib: u64, into: &Volume) -> ApiResult<()> {
        if into.spec.size_gib <= source_gib {
            return Ok(());
        }
        run(
            &super::configured_bin(ENV_BIN, "rbd"),
            &[
                "resize".into(),
                "--size".into(),
                format!("{}G", into.spec.size_gib),
                image.to_owned(),
            ],
            false,
        )
        .await
    }

    /// `qemu-img convert -O raw -f <format> -n <src> rbd:{pool}/{id}`.
    fn image_convert_args(&self, vol: &Volume, src: &Path, format: &str) -> ApiResult<Vec<String>> {
        let pool = self.pool()?;
        let id = vol_id(vol)?;
        Ok(super::qemu_img_convert_argv(
            format,
            src,
            &format!("rbd:{pool}/{id}"),
        ))
    }
}

#[async_trait]
impl BlockBackend for RbdBackend {
    fn name(&self) -> &'static str {
        "rbd"
    }

    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef> {
        blank_only(vol)?;
        self.require_available().await?;
        let pool = self.pool()?;
        let id = vol_id(vol)?;
        let size = format!("{}G", vol.spec.size_gib);
        let image = format!("{pool}/{id}");
        run(
            &super::configured_bin(ENV_BIN, "rbd"),
            &[
                "create".into(),
                "--size".into(),
                size,
                "--image-feature".into(),
                IMAGE_FEATURES.into(),
                image.clone(),
            ],
            false,
        )
        .await?;
        Ok(BackendRef::new(image))
    }

    async fn write_image(&self, vol: &Volume, src: &Path, format: &str) -> ApiResult<()> {
        let args = self.image_convert_args(vol, src, format)?;
        super::run_qemu_img(src, &args).await
    }

    async fn delete(&self, vol: &Volume) -> ApiResult<()> {
        self.require_available().await?;
        let pool = self.pool()?;
        let id = vol_id(vol)?;
        run(
            &super::configured_bin(ENV_BIN, "rbd"),
            &["rm".into(), format!("{pool}/{id}")],
            true,
        )
        .await
    }

    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef> {
        self.require_available().await?;
        let pool = self.pool()?;
        let vol_id = vol_id(vol)?;
        let snap_id = snap_id(snap)?;
        let spec = format!("{pool}/{vol_id}@{snap_id}");
        let bin = super::configured_bin(ENV_BIN, "rbd");
        run(&bin, &["snap".into(), "create".into(), spec.clone()], false).await?;
        run(
            &bin,
            &["snap".into(), "protect".into(), spec.clone()],
            false,
        )
        .await?;
        Ok(BackendRef::new(spec))
    }

    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()> {
        self.require_available().await?;
        let pool = self.pool()?;
        let vol_id = snap.spec.volume.to_string();
        ident("volume id", &vol_id)?;
        let snap_id = snap_id(snap)?;
        let spec = format!("{pool}/{vol_id}@{snap_id}");
        let bin = super::configured_bin(ENV_BIN, "rbd");
        run(
            &bin,
            &["snap".into(), "unprotect".into(), spec.clone()],
            true,
        )
        .await?;
        run(&bin, &["snap".into(), "rm".into(), spec], true).await
    }

    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef> {
        self.require_available().await?;
        let pool = self.pool()?;
        let src = snap.spec.volume.to_string();
        ident("volume id", &src)?;
        let snap_id = snap_id(snap)?;
        let id = vol_id(into)?;
        let image = format!("{pool}/{id}");
        run(
            &super::configured_bin(ENV_BIN, "rbd"),
            &[
                "clone".into(),
                format!("{pool}/{src}@{snap_id}"),
                image.clone(),
            ],
            false,
        )
        .await?;
        self.grow(&image, snap.status.size_gib, into).await?;
        Ok(BackendRef::new(image))
    }

    async fn clone_volume(&self, src: &Volume, into: &Volume) -> ApiResult<BackendRef> {
        self.require_available().await?;
        let pool = self.pool()?;
        let src_id = vol_id(src)?;
        let id = vol_id(into)?;
        let image = format!("{pool}/{id}");
        run_copy(
            &super::configured_bin(ENV_BIN, "rbd"),
            &[
                "cp".into(),
                "--image-feature".into(),
                "layering,exclusive-lock".into(),
                format!("{pool}/{src_id}"),
                image.clone(),
            ],
        )
        .await?;
        self.grow(&image, src.spec.size_gib, into).await?;
        Ok(BackendRef::new(image))
    }

    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()> {
        if new_size_gib < vol.spec.size_gib {
            return Err(ApiError::unprocessable("volumes cannot shrink"));
        }
        self.require_available().await?;
        let pool = self.pool()?;
        let id = vol_id(vol)?;
        run(
            &super::configured_bin(ENV_BIN, "rbd"),
            &[
                "resize".into(),
                "--size".into(),
                format!("{new_size_gib}G"),
                format!("{pool}/{id}"),
            ],
            false,
        )
        .await
    }

    async fn attach_info(&self, vol: &Volume, _host: &str) -> ApiResult<AttachInfo> {
        self.require_available().await?;
        let pool = self.pool()?.to_owned();
        let id = vol_id(vol)?;
        let mut info = AttachInfo::new(AttachTarget::Rbd {
            pool,
            image: id,
            monitors: self.monitors.clone(),
            user: self.user(),
            key_secret: None,
        });
        info.device = vol.status.device.clone();
        Ok(info)
    }

    async fn bytes_path(&self, vol: &Volume) -> ApiResult<std::path::PathBuf> {
        let _ = vol;
        Err(ApiError::unavailable("rbd backup is not available"))
    }

    async fn open_backup(&self, vol: &Volume) -> ApiResult<BackupStream> {
        self.require_available().await?;
        let pool = self.pool()?.to_owned();
        let id = vol_id(vol)?;
        let spec = format!("{pool}/{id}");
        let bin = super::configured_bin(ENV_BIN, "rbd");
        BackupStream::spawn(&bin, &["export".into(), spec, "-".into()], "rbd export")
    }

    async fn open_restore(&self, vol: &Volume) -> ApiResult<RestoreStream> {
        self.require_available().await?;
        let pool = self.pool()?.to_owned();
        let id = vol_id(vol)?;
        let staging_id = format!("{id}.restore");
        ident("volume id", &staging_id)?;
        let image = format!("{pool}/{id}");
        let staging = format!("{pool}/{staging_id}");
        let bin = super::configured_bin(ENV_BIN, "rbd");

        let snaps = run_out(
            &bin,
            &[
                "snap".into(),
                "ls".into(),
                "--format".into(),
                "json".into(),
                image.clone(),
            ],
            true,
        )
        .await?;
        if !matches!(snaps.trim(), "" | "[]") {
            return Err(ApiError::conflict(format!(
                "volume {id} has snapshots; restore the backup into a new volume"
            )));
        }
        // Left behind by a restore that did not finish.
        run(&bin, &["rm".into(), staging.clone()], true).await?;

        let want = super::gib_to_bytes(vol.spec.size_gib);
        let size = format!("{}G", vol.spec.size_gib);
        let after_bin = bin.clone();
        let after: super::RestoreFinish = Box::new(move |written| {
            Box::pin(async move {
                let bin = after_bin;
                if let Err(e) = run(&bin, &["rm".into(), image.clone()], true).await {
                    // The volume is as it was. Do not leave the copy around.
                    if let Err(del) = run(&bin, &["rm".into(), staging.clone()], true).await {
                        tracing::warn!(image = %staging, error = %del, "removing a restore image failed");
                    }
                    return Err(e);
                }
                run(&bin, &["rename".into(), staging, image.clone()], false).await?;
                if written < want {
                    run(
                        &bin,
                        &["resize".into(), "--size".into(), size, image],
                        false,
                    )
                    .await?;
                }
                Ok(())
            })
        });
        RestoreStream::spawn(
            &bin,
            &[
                "import".into(),
                "--image-feature".into(),
                IMAGE_FEATURES.into(),
                "-".into(),
                format!("{pool}/{staging_id}"),
            ],
            "rbd import",
            after,
        )
    }
}

fn vol_id(vol: &Volume) -> ApiResult<String> {
    let id = vol.meta.id.to_string();
    ident("volume id", &id)?;
    Ok(id)
}

/// `create` makes a blank image. Refuse a source it would drop.
fn blank_only(vol: &Volume) -> ApiResult<()> {
    use crate::types::VolumeSource;
    match vol.spec.source {
        VolumeSource::None | VolumeSource::Image(_) => Ok(()),
        VolumeSource::Snapshot(_) | VolumeSource::Volume(_) => Err(ApiError::internal(
            "rbd create makes a blank image; a snapshot or volume source is restored or cloned",
        )),
    }
}

fn snap_id(snap: &Snapshot) -> ApiResult<String> {
    let id = snap.meta.id.to_string();
    ident("snapshot id", &id)?;
    Ok(id)
}

fn ident(kind: &str, value: &str) -> ApiResult<()> {
    let ok = (1..=64).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'));
    if ok {
        Ok(())
    } else {
        Err(ApiError::bad_request(format!(
            "{kind} must match [A-Za-z0-9._-]{{1,64}}"
        )))
    }
}

fn split_monitors(raw: Option<&String>) -> Vec<String> {
    raw.map(|raw| {
        raw.split(',')
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .collect()
    })
    .unwrap_or_default()
}

fn parse_bool(raw: &str) -> Option<bool> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "1" | "yes" => Some(true),
        "false" | "0" | "no" | "" => Some(false),
        _ => None,
    }
}

async fn run(bin: &str, args: &[String], missing_ok: bool) -> ApiResult<()> {
    run_out(bin, args, missing_ok).await.map(|_| ())
}

/// Run a command and return its stdout. With `missing_ok`, a command that
/// failed because the image or snapshot is gone returns an empty string.
async fn run_out(bin: &str, args: &[String], missing_ok: bool) -> ApiResult<String> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let finished = tokio::time::timeout(TIMEOUT, cmd.output()).await;
    let output = match finished {
        Err(_) => return Err(ApiError::unavailable(format!("{bin} timed out after 30s"))),
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::unavailable(format!("{bin} is not available")))
        }
        Ok(Err(e)) => return Err(ApiError::internal(format!("{bin}: {e}"))),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(String::from_utf8_lossy(&output.stdout).into_owned());
    }
    let detail = format!("{}{}", clip(&output.stderr), clip(&output.stdout));
    if missing_ok && object_absent(&detail) {
        return Ok(String::new());
    }
    let code = output.status.code().unwrap_or(-1);
    Err(ApiError::internal(format!("{bin} exited {code}: {detail}")))
}

fn clip(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .trim()
        .chars()
        .take(400)
        .collect()
}

fn object_absent(text: &str) -> bool {
    let s = text.to_ascii_lowercase();
    s.contains("no such file or directory")
        || s.contains("not protected")
        || s.contains("already unprotected")
        || (s.contains("not found") && (s.contains("image") || s.contains("snap")))
}

/// `rbd cp` reads and writes every allocated block of the source, so it
/// gets the limit an image copy has instead of the 30s one.
const COPY_TIMEOUT: Duration = Duration::from_secs(60 * 60);

async fn run_copy(bin: &str, args: &[String]) -> ApiResult<()> {
    let mut cmd = tokio::process::Command::new(bin);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    let finished = tokio::time::timeout(COPY_TIMEOUT, cmd.output()).await;
    let output = match finished {
        Err(_) => {
            return Err(ApiError::unavailable(format!(
                "{bin} cp timed out after {}s",
                COPY_TIMEOUT.as_secs()
            )))
        }
        Ok(Err(e)) if e.kind() == std::io::ErrorKind::NotFound => {
            return Err(ApiError::unavailable(format!("{bin} is not available")))
        }
        Ok(Err(e)) => return Err(ApiError::internal(format!("{bin}: {e}"))),
        Ok(Ok(output)) => output,
    };
    if output.status.success() {
        return Ok(());
    }
    let detail = format!("{}{}", clip(&output.stderr), clip(&output.stdout));
    let code = output.status.code().unwrap_or(-1);
    Err(ApiError::internal(format!("{bin} exited {code}: {detail}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{VolumeSpec, VolumeStatus};
    use pistack_types::{kind, Meta};
    use tokio::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::const_new(());

    fn volume(size: u64) -> Volume {
        Volume {
            meta: Meta::new(kind::VOLUME, "data", None),
            spec: VolumeSpec {
                size_gib: size,
                volume_type: "rbd".into(),
                source: Default::default(),
                bootable: false,
                encrypted: false,
            },
            status: VolumeStatus::default(),
        }
    }

    #[tokio::test]
    async fn create_argv_contains_id_and_size() {
        let _guard = LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("rbd");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\nexit 0\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(ENV_BIN, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(ENV_BIN);
            }
        }
        let _reset = Reset;

        let bad = RbdBackend::new("bad/pool", "pistack", Vec::new());
        let vol = volume(17);
        let err = bad.create(&vol).await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);
        assert!(!log.exists());

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let id = vol.meta.id.to_string();
        backend.create(&vol).await.unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains(&id), "{logged}");
        assert!(logged.contains("17G"), "{logged}");

        std::env::set_var(ENV_BIN, dir.path().join("missing-rbd"));
        let err = backend.create(&vol).await.unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
    }

    #[tokio::test]
    async fn missing_monitors_or_pool_does_not_create() {
        let _guard = LOCK.lock().await;
        let vol = volume(4);
        let err = RbdBackend::new("volumes", "pistack", Vec::new())
            .create(&vol)
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
        assert!(err.message.contains("monitor"), "{err}");

        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("rbd");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\nif [ \"$1\" = \"pool\" ]; then exit 1; fi\nexit 0\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(ENV_BIN, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(ENV_BIN);
            }
        }
        let _reset = Reset;

        let err = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()])
            .create(&vol)
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
        assert!(err.message.contains("not available"), "{err}");
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(!logged.lines().any(|line| line == "create"), "{logged}");
    }

    #[tokio::test]
    async fn restore_and_clone_carry_the_source() {
        let _guard = LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("rbd");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{quoted}'\nexit 0\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(ENV_BIN, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(ENV_BIN);
            }
        }
        let _reset = Reset;
        // One line per command, without the `rbd pool stats` probe.
        let issued = || -> Vec<String> {
            let logged = std::fs::read_to_string(&log).unwrap_or_default();
            let _ = std::fs::remove_file(&log);
            logged
                .lines()
                .filter(|line| !line.starts_with("pool stats"))
                .map(str::to_owned)
                .collect()
        };

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let src = volume(4);
        let src_id = src.meta.id.to_string();
        let snap = Snapshot {
            meta: Meta::new(kind::SNAPSHOT, "snap", None),
            spec: crate::types::SnapshotSpec {
                volume: src.meta.id.clone(),
            },
            status: crate::types::SnapshotStatus {
                size_gib: 4,
                ..Default::default()
            },
        };
        let snap_id = snap.meta.id.to_string();

        // Larger than the snapshot: cloned, then grown to the requested size.
        let mut restored = volume(6);
        restored.spec.source = crate::types::VolumeSource::Snapshot(snap.meta.id.clone());
        let id = restored.meta.id.to_string();
        let made = backend.restore(&snap, &restored).await.unwrap();
        assert_eq!(made.as_str(), format!("volumes/{id}"));
        assert_eq!(
            issued(),
            vec![
                format!("clone volumes/{src_id}@{snap_id} volumes/{id}"),
                format!("resize --size 6G volumes/{id}"),
            ]
        );

        // Same size as the source volume: copied, not resized.
        let mut cloned = volume(4);
        cloned.spec.source = crate::types::VolumeSource::Volume(src.meta.id.clone());
        let id = cloned.meta.id.to_string();
        let made = backend.clone_volume(&src, &cloned).await.unwrap();
        assert_eq!(made.as_str(), format!("volumes/{id}"));
        assert_eq!(
            issued(),
            vec![format!(
                "cp --image-feature layering,exclusive-lock volumes/{src_id} volumes/{id}"
            )]
        );

        // `create` would drop the source, so it refuses and runs nothing.
        for vol in [&restored, &cloned] {
            let err = backend.create(vol).await.unwrap_err();
            assert_eq!(err.code, pistack_types::error::ErrorCode::Internal);
            assert!(err.message.contains("blank"), "{err}");
        }
        assert!(issued().is_empty());
    }

    #[tokio::test]
    async fn image_convert_command_targets_rbd() {
        let _guard = LOCK.lock().await;
        let _qemu = super::super::QEMU_IMG_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("qemu-img");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!("#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\nexit 0\n"),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(super::super::ENV_QEMU_IMG, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(super::super::ENV_QEMU_IMG);
            }
        }
        let _reset = Reset;

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let vol = volume(8);
        let src = dir.path().join("seed.qcow2");
        std::fs::write(&src, b"seed").unwrap();
        let id = vol.meta.id.to_string();
        let args = backend.image_convert_args(&vol, &src, "qcow2").unwrap();
        assert_eq!(
            args,
            vec![
                "convert".to_owned(),
                "-O".to_owned(),
                "raw".to_owned(),
                "-f".to_owned(),
                "qcow2".to_owned(),
                "-n".to_owned(),
                src.display().to_string(),
                format!("rbd:volumes/{id}"),
            ]
        );
        assert!(!args.iter().any(|a| a == "import"));
        backend.write_image(&vol, &src, "qcow2").await.unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            logged,
            args.iter().map(|a| format!("{a}\n")).collect::<String>()
        );
    }

    #[tokio::test]
    async fn backup_export_streams_stdout() {
        let _guard = LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("rbd");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\n\
                 if [ \"$1\" = \"export\" ]; then printf 'xyz'; fi\nexit 0\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(ENV_BIN, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(ENV_BIN);
            }
        }
        let _reset = Reset;

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let vol = volume(2);
        let id = vol.meta.id.to_string();
        let mut stream = backend.open_backup(&vol).await.unwrap();
        let mut buf = [0u8; 8];
        let n = stream.read(&mut buf).await.unwrap();
        assert_eq!(&buf[..n], b"xyz");
        stream.finish().await.unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains("export"), "{logged}");
        assert!(logged.contains(&format!("volumes/{id}")), "{logged}");
        assert!(logged.lines().any(|line| line == "-"), "{logged}");
    }

    /// Write `body` as an executable `name` in `dir` that first appends its
    /// arguments, one invocation per line, to `<name>.log`.
    fn script(dir: &Path, name: &str, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let log = dir.join(format!("{name}.log"));
        let bin = dir.join(name);
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &bin,
            format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{quoted}'\n{body}\n"),
        )
        .unwrap();
        std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();
        (bin, log)
    }

    struct ResetBins;
    impl Drop for ResetBins {
        fn drop(&mut self) {
            std::env::remove_var(ENV_BIN);
            std::env::remove_var(ENV_CEPH_BIN);
        }
    }

    #[tokio::test]
    async fn restore_refuses_a_volume_with_snapshots() {
        let _guard = LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = script(
            dir.path(),
            "rbd",
            "if [ \"$1\" = \"snap\" ]; then printf '[{\"id\":4,\"name\":\"nightly\"}]\\n'; fi\nexit 0",
        );
        std::env::set_var(ENV_BIN, &bin);
        let _reset = ResetBins;

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let vol = volume(2);
        let err = backend.open_restore(&vol).await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Conflict, "{err}");
        assert!(err.message.contains("snapshots"), "{err}");
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(!logged.contains("import"), "{logged}");
        assert!(!logged.lines().any(|l| l.starts_with("rm ")), "{logged}");
    }

    #[tokio::test]
    async fn a_failed_import_leaves_the_volume_in_place() {
        let _guard = LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let (bin, log) = script(
            dir.path(),
            "rbd",
            "case \"$1\" in\n  snap) printf '[]\\n' ;;\n  import) cat > /dev/null; echo 'no space' >&2; exit 28 ;;\nesac\nexit 0",
        );
        std::env::set_var(ENV_BIN, &bin);
        let _reset = ResetBins;

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let vol = volume(2);
        let id = vol.meta.id.to_string();
        let mut stream = backend.open_restore(&vol).await.unwrap();
        stream.write_all(b"abc").await.unwrap();
        let err = stream.finish().await.unwrap_err();
        assert!(err.message.contains("rbd import failed"), "{err}");
        assert!(err.message.contains("no space"), "{err}");
        let logged = std::fs::read_to_string(&log).unwrap();
        // Only the leftover of an earlier restore is ever removed.
        assert!(
            !logged.lines().any(|l| l == format!("rm volumes/{id}")),
            "{logged}"
        );
        assert!(!logged.contains("rename"), "{logged}");
    }

    #[tokio::test]
    async fn the_pool_is_created_only_when_asked() {
        let _guard = LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let made = dir.path().join("pool-made");
        let quoted = made.display().to_string().replace('\'', "'\\''");
        let (bin, log) = script(
            dir.path(),
            "rbd",
            &format!(
                "if [ \"$1 $2\" = \"pool init\" ]; then : > '{quoted}'; fi\n\
                 if [ \"$1 $2\" = \"pool stats\" ] && [ ! -e '{quoted}' ]; then exit 2; fi\nexit 0"
            ),
        );
        let (ceph, ceph_log) = script(dir.path(), "ceph", "exit 0");
        std::env::set_var(ENV_BIN, &bin);
        std::env::set_var(ENV_CEPH_BIN, &ceph);
        let _reset = ResetBins;

        let backend = RbdBackend::new("volumes", "pistack", vec!["10.0.0.1:6789".into()]);
        let err = backend.require_available().await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(
            err.message.contains("ceph osd pool create volumes"),
            "{err}"
        );
        assert!(err.message.contains("rbd pool init volumes"), "{err}");
        assert!(!ceph_log.exists(), "ceph must not run unless asked");

        let mut params = BTreeMap::new();
        params.insert("monitors".to_owned(), "10.0.0.1:6789".to_owned());
        params.insert("create_pool".to_owned(), "true".to_owned());
        RbdBackend::validate_params(&params).unwrap();
        let backend = RbdBackend::from_params(&params);
        backend.require_available().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&ceph_log).unwrap(),
            "osd pool create volumes\n"
        );
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.lines().any(|l| l == "pool init volumes"), "{logged}");

        // The pool is there now, so nothing is created a second time.
        backend.require_available().await.unwrap();
        assert_eq!(
            std::fs::read_to_string(&ceph_log).unwrap(),
            "osd pool create volumes\n"
        );

        std::env::set_var(ENV_CEPH_BIN, dir.path().join("missing-ceph"));
        std::fs::remove_file(&made).unwrap();
        let err = backend.require_available().await.unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable, "{err}");
        assert!(err.message.contains("ceph is not available"), "{err}");
    }
}
