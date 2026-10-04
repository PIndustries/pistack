// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Storage backends. `slab` decides *what* exists; a `BlockBackend` makes it
//! exist on real storage and tells `anvil` how to reach it.
//!
//! The volume type selects the backend. Empty or `local` is the sparse file
//! backend and stays the default. `lvm` runs only when the volume group, thin
//! pool, and LVM tools exist. `rbd` runs only when the pool, monitors, and
//! `rbd` binary exist. A missing binary or pool is unavailable.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};
use serde::{Deserialize, Serialize};

use crate::types::{Snapshot, Volume};

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

    /// Create storage for `vol` (blank, or populated from `spec.source` when
    /// the backend can do so natively). Returns the handle to store.
    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef>;

    /// Delete the storage behind `vol`. Must be idempotent.
    async fn delete(&self, vol: &Volume) -> ApiResult<()>;

    /// Take a point-in-time snapshot of `vol`.
    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef>;

    /// Delete a snapshot. Must be idempotent.
    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()>;

    /// Materialise `snap` as the new volume `into`.
    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef>;

    /// Grow `vol` to `new_size_gib`. Shrinking is never allowed.
    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()>;

    /// How `host` should attach `vol`.
    async fn attach_info(&self, vol: &Volume, host: &str) -> ApiResult<AttachInfo>;

    /// File that holds the volume bytes for a backup. Backends without a
    /// file in the slab directory are unavailable.
    async fn bytes_path(&self, vol: &Volume) -> ApiResult<std::path::PathBuf>;
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

fn is_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}
