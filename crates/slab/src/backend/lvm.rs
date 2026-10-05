// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! LVM thin-pool backend.
//!
//! `$PISTACK_LVM_BIN`, when set and non-empty, replaces the `lvcreate` /
//! `lvs` family binary (`lvcreate`, `lvremove`, `lvchange`, `lvextend`).
//! Otherwise those names are looked up on `PATH`. A missing binary is
//! unavailable. `vg`, `pool`, volume ids and snapshot ids must match
//! `[A-Za-z0-9._-]{1,64}`.
//!
//! | operation       | command |
//! | --------------- | ------- |
//! | create          | `lvcreate --thin --virtualsize {size_gib}G --name {vol_id} {vg}/{pool}` |
//! | delete          | `lvremove --force {vg}/{vol_id}` |
//! | snapshot        | `lvcreate --snapshot --name {snap_id} {vg}/{vol_id}` (thin snapshots need no size) |
//! | delete_snapshot | `lvremove --force {vg}/{snap_id}` |
//! | restore         | `lvcreate --snapshot --name {new_vol_id} {vg}/{snap_id}` then `lvchange -ay -K {vg}/{new_vol_id}` |
//! | extend          | `lvextend --size {new_size_gib}G {vg}/{vol_id}` |
//! | attach_info     | same host: `Local { path: /dev/{vg}/{vol_id} }` |
//!
//! Parameters come from `VolumeTypeSpec::params`: `vg` (required),
//! `pool` (thin pool, default `pistack-thin`).
//!
//! A mutating command runs only after `vgs` shows the volume group and
//! `lvs` shows the thin pool, and only when `lvcreate`, `lvremove`,
//! `lvchange`, `lvextend`, `vgs`, and `lvs` are executable. A missing
//! binary, volume group, or thin pool is unavailable. This backend does
//! not create the group or the pool.

use async_trait::async_trait;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiError, ApiResult};
use std::collections::BTreeMap;
use std::time::Duration;

use super::{AttachInfo, AttachTarget, BackendRef, BlockBackend};
use crate::types::{Snapshot, Volume};

const TIMEOUT: Duration = Duration::from_secs(30);
const ENV_BIN: &str = "PISTACK_LVM_BIN";

#[derive(Default)]
pub struct LvmBackend {
    pub vg: String,
    pub pool: String,
}

impl LvmBackend {
    pub fn new(vg: impl Into<String>, pool: impl Into<String>) -> Self {
        Self {
            vg: vg.into(),
            pool: pool.into(),
        }
    }

    /// `vg` and `pool` from a volume type. An empty pool stays empty here
    /// and defaults to `pistack-thin` when a command is built.
    pub fn from_params(params: &BTreeMap<String, String>) -> Self {
        Self {
            vg: params.get("vg").cloned().unwrap_or_default(),
            pool: params.get("pool").cloned().unwrap_or_default(),
        }
    }

    /// Tools, volume group, and thin pool must all exist. No LVM command
    /// that creates or removes a logical volume runs before this returns.
    pub async fn require_available(&self) -> ApiResult<()> {
        let vg = self.vg()?.to_owned();
        let pool = self.pool()?.to_owned();
        for tool in ["vgs", "lvs", "lvcreate", "lvremove", "lvchange", "lvextend"] {
            super::configured_bin_ready(ENV_BIN, tool)?;
        }
        let vgs = super::configured_bin(ENV_BIN, "vgs");
        run(
            &vgs,
            &[
                "--noheadings".into(),
                "-o".into(),
                "vg_name".into(),
                vg.clone(),
            ],
            false,
        )
        .await
        .map_err(|e| unavailable_probe(e, &format!("volume group {vg}")))?;
        let lvs = super::configured_bin(ENV_BIN, "lvs");
        run(
            &lvs,
            &[
                "--noheadings".into(),
                "-o".into(),
                "lv_name".into(),
                format!("{vg}/{pool}"),
            ],
            false,
        )
        .await
        .map_err(|e| unavailable_probe(e, &format!("thin pool {vg}/{pool}")))?;
        Ok(())
    }

    fn vg(&self) -> ApiResult<&str> {
        if self.vg.is_empty() {
            return Err(ApiError::unavailable("volume group is not configured"));
        }
        ident("vg", &self.vg)?;
        Ok(&self.vg)
    }

    fn pool(&self) -> ApiResult<&str> {
        let pool = if self.pool.is_empty() {
            "pistack-thin"
        } else {
            self.pool.as_str()
        };
        ident("pool", pool)?;
        Ok(pool)
    }
}

#[async_trait]
impl BlockBackend for LvmBackend {
    fn name(&self) -> &'static str {
        "lvm"
    }

    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef> {
        self.require_available().await?;
        let vg = self.vg()?;
        let pool = self.pool()?;
        let id = vol_id(vol)?;
        let size = format!("{}G", vol.spec.size_gib);
        run(
            &super::configured_bin(ENV_BIN, "lvcreate"),
            &[
                "--thin".into(),
                "--virtualsize".into(),
                size,
                "--name".into(),
                id.clone(),
                format!("{vg}/{pool}"),
            ],
            false,
        )
        .await?;
        Ok(BackendRef::new(format!("/dev/{vg}/{id}")))
    }

    async fn delete(&self, vol: &Volume) -> ApiResult<()> {
        self.require_available().await?;
        let vg = self.vg()?;
        let id = vol_id(vol)?;
        run(
            &super::configured_bin(ENV_BIN, "lvremove"),
            &["--force".into(), format!("{vg}/{id}")],
            true,
        )
        .await
    }

    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef> {
        self.require_available().await?;
        let vg = self.vg()?;
        let vol_id = vol_id(vol)?;
        let snap_id = snap_id(snap)?;
        run(
            &super::configured_bin(ENV_BIN, "lvcreate"),
            &[
                "--snapshot".into(),
                "--name".into(),
                snap_id.clone(),
                format!("{vg}/{vol_id}"),
            ],
            false,
        )
        .await?;
        Ok(BackendRef::new(format!("/dev/{vg}/{snap_id}")))
    }

    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()> {
        self.require_available().await?;
        let vg = self.vg()?;
        let id = snap_id(snap)?;
        run(
            &super::configured_bin(ENV_BIN, "lvremove"),
            &["--force".into(), format!("{vg}/{id}")],
            true,
        )
        .await
    }

    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef> {
        self.require_available().await?;
        let vg = self.vg()?;
        let snap_id = snap_id(snap)?;
        let id = vol_id(into)?;
        run(
            &super::configured_bin(ENV_BIN, "lvcreate"),
            &[
                "--snapshot".into(),
                "--name".into(),
                id.clone(),
                format!("{vg}/{snap_id}"),
            ],
            false,
        )
        .await?;
        run(
            &super::configured_bin(ENV_BIN, "lvchange"),
            &["-ay".into(), "-K".into(), format!("{vg}/{id}")],
            false,
        )
        .await?;
        Ok(BackendRef::new(format!("/dev/{vg}/{id}")))
    }

    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()> {
        if new_size_gib < vol.spec.size_gib {
            return Err(ApiError::unprocessable("volumes cannot shrink"));
        }
        self.require_available().await?;
        let vg = self.vg()?;
        let id = vol_id(vol)?;
        run(
            &super::configured_bin(ENV_BIN, "lvextend"),
            &[
                "--size".into(),
                format!("{new_size_gib}G"),
                format!("{vg}/{id}"),
            ],
            false,
        )
        .await
    }

    async fn attach_info(&self, vol: &Volume, _host: &str) -> ApiResult<AttachInfo> {
        self.require_available().await?;
        let vg = self.vg()?;
        let id = vol_id(vol)?;
        let mut info = AttachInfo::new(AttachTarget::Local {
            path: format!("/dev/{vg}/{id}"),
        });
        info.device = vol.status.device.clone();
        Ok(info)
    }

    async fn bytes_path(&self, vol: &Volume) -> ApiResult<std::path::PathBuf> {
        let _ = vol;
        Err(ApiError::unavailable("lvm backup is not available"))
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

fn unavailable_probe(err: ApiError, what: &str) -> ApiError {
    if err.code == ErrorCode::Unavailable {
        err
    } else {
        ApiError::unavailable(format!("{what} is not available"))
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
    s.contains("failed to find logical volume")
        || s.contains("failed to find device")
        || s.contains("no such file or directory")
        || (s.contains("not found") && (s.contains("image") || s.contains("snap")))
        || s.contains("not protected")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{VolumeSpec, VolumeStatus};
    use pistack_types::{kind, Meta};
    use std::sync::Mutex;

    static LOCK: Mutex<()> = Mutex::new(());

    fn volume(size: u64) -> Volume {
        Volume {
            meta: Meta::new(kind::VOLUME, "data", None),
            spec: VolumeSpec {
                size_gib: size,
                volume_type: "lvm".into(),
                source: Default::default(),
                bootable: false,
                encrypted: false,
            },
            status: VolumeStatus::default(),
        }
    }

    #[tokio::test]
    async fn create_argv_contains_id_and_size() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("lvm");
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

        let bad = LvmBackend::new("bad/vg", "pistack-thin");
        let vol = volume(17);
        let err = bad.create(&vol).await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);
        assert!(!log.exists());

        let backend = LvmBackend::new("vg0", "pistack-thin");
        let id = vol.meta.id.to_string();
        backend.create(&vol).await.unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(logged.contains(&id), "{logged}");
        assert!(logged.contains("17G"), "{logged}");

        std::env::set_var(ENV_BIN, dir.path().join("missing-lvm"));
        let err = backend.create(&vol).await.unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
    }

    #[tokio::test]
    async fn missing_thin_pool_does_not_create() {
        let _guard = LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("lvm");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\n\
                 if [ \"$1\" = \"--noheadings\" ]; then\n\
                   last=\n\
                   for a in \"$@\"; do last=$a; done\n\
                   case \"$last\" in */*) exit 1 ;; esac\n\
                 fi\nexit 0\n"
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

        let err = LvmBackend::new("", "pistack-thin")
            .create(&volume(1))
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
        assert!(!log.exists(), "empty vg must not run a command");

        let err = LvmBackend::new("vg0", "pistack-thin")
            .create(&volume(1))
            .await
            .unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
        assert!(err.message.contains("not available"), "{err}");
        let logged = std::fs::read_to_string(&log).unwrap();
        assert!(!logged.contains("--thin"), "{logged}");
        assert!(!logged.contains("--virtualsize"), "{logged}");
    }
}
