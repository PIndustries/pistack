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
//! | restore         | `rbd clone {pool}/{vol_id}@{snap_id} {pool}/{new_vol_id}` |
//! | extend          | `rbd resize --size {new_size_gib}G {pool}/{vol_id}` |
//! | attach_info     | `Rbd { pool, image: vol_id, monitors, user }` |
//! | image           | `qemu-img convert -O raw -f <fmt> -n <src> rbd:{pool}/{vol_id}` |
//! | backup          | `rbd export {pool}/{vol_id} -` (stdout is the byte stream) |
//! | backup restore  | unavailable; an existing image has no stdin import |
//!
//! The image is created with `rbd create` first. `qemu-img convert -n`
//! then writes raw bytes into that image (`-n` is qemu-img's "do not
//! create the target", and its help cites rbd). `rbd import` would store
//! a qcow2 file's bytes unchanged and would not fill the image `rbd
//! create` just made. Monitors stay in the host ceph config, the same way
//! `rbd create` is invoked.
//!
//! Parameters from `VolumeTypeSpec::params`: `pool` (default `volumes`),
//! `user` (default `pistack`), `monitors` (comma separated).
//!
//! A mutating command runs only when monitors are configured, `rbd` is
//! executable, and `rbd pool stats {pool}` succeeds. A missing binary,
//! monitor list, or pool is unavailable. This backend does not create
//! the pool.

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

pub struct RbdBackend {
    pub pool: String,
    pub user: String,
    pub monitors: Vec<String>,
}

impl Default for RbdBackend {
    fn default() -> Self {
        Self {
            pool: "volumes".into(),
            user: "pistack".into(),
            monitors: Vec::new(),
        }
    }
}

impl RbdBackend {
    pub fn new(pool: impl Into<String>, user: impl Into<String>, monitors: Vec<String>) -> Self {
        Self {
            pool: pool.into(),
            user: user.into(),
            monitors,
        }
    }

    /// `pool`, `user`, and comma-separated `monitors` from a volume type.
    pub fn from_params(params: &BTreeMap<String, String>) -> Self {
        let monitors = params
            .get("monitors")
            .map(|raw| {
                raw.split(',')
                    .map(str::trim)
                    .filter(|m| !m.is_empty())
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        Self {
            pool: params.get("pool").cloned().unwrap_or_default(),
            user: params.get("user").cloned().unwrap_or_default(),
            monitors,
        }
    }

    /// Monitors, the `rbd` binary, and the pool must all exist. No `rbd`
    /// create, remove, clone, or resize runs before this returns.
    pub async fn require_available(&self) -> ApiResult<()> {
        let pool = self.pool()?.to_owned();
        if !self.monitors.iter().any(|m| !m.trim().is_empty()) {
            return Err(ApiError::unavailable("rbd monitors are not configured"));
        }
        let bin = super::configured_bin_ready(ENV_BIN, "rbd")?;
        run(&bin, &["pool".into(), "stats".into(), pool.clone()], false)
            .await
            .map_err(|e| {
                if e.code == ErrorCode::Unavailable {
                    e
                } else {
                    ApiError::unavailable(format!("rbd pool {pool} is not available"))
                }
            })?;
        Ok(())
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
                "layering,exclusive-lock".into(),
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
        let _ = vol;
        Err(ApiError::unavailable("rbd backup restore is not available"))
    }
}

fn vol_id(vol: &Volume) -> ApiResult<String> {
    let id = vol.meta.id.to_string();
    ident("volume id", &id)?;
    Ok(id)
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

async fn run(bin: &str, args: &[String], missing_ok: bool) -> ApiResult<()> {
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
        return Ok(());
    }
    let detail = format!("{}{}", clip(&output.stderr), clip(&output.stdout));
    if missing_ok && object_absent(&detail) {
        return Ok(());
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

        let err = backend.open_restore(&vol).await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unavailable);
        assert!(err.message.contains("restore"), "{err}");
    }
}
