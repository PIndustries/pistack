// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Ceph RBD backend. Stub: every method returns `not_implemented`.
//!
//! When wired, the backend runs these (control plane or any host with a
//! Ceph keyring; `anvil` does the `rbd map`/librbd attach side):
//!
//! | operation       | command |
//! | --------------- | ------- |
//! | create          | `rbd create --size {size_gib}G --image-feature layering,exclusive-lock {pool}/{vol_id}` |
//! | delete          | `rbd rm {pool}/{vol_id}` |
//! | snapshot        | `rbd snap create {pool}/{vol_id}@{snap_id}` then `rbd snap protect {pool}/{vol_id}@{snap_id}` |
//! | delete_snapshot | `rbd snap unprotect {pool}/{vol_id}@{snap_id}` then `rbd snap rm {pool}/{vol_id}@{snap_id}` |
//! | restore         | `rbd clone {pool}/{vol_id}@{snap_id} {pool}/{new_vol_id}` (flatten later with `rbd flatten`) |
//! | extend          | `rbd resize --size {new_size_gib}G {pool}/{vol_id}` |
//! | attach_info     | `Rbd { pool, image: vol_id, monitors, user }`; anvil runs `rbd map {pool}/{vol_id}` for bare metal or hands librbd to libvirt for VMs |
//!
//! Parameters from `VolumeTypeSpec::params`: `pool` (default `volumes`),
//! `user` (default `pistack`), `monitors` (comma separated), `keyring_secret`
//! (a `keep` secret id holding the cephx key).

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};

use super::{AttachInfo, BackendRef, BlockBackend};
use crate::types::{Snapshot, Volume};

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

    fn todo(op: &str, cmd: String) -> ApiError {
        ApiError::not_implemented(format!("rbd backend: {op} not implemented"))
            .with_detail("command", cmd)
    }
}

#[async_trait]
impl BlockBackend for RbdBackend {
    fn name(&self) -> &'static str {
        "rbd"
    }

    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef> {
        Err(Self::todo(
            "create",
            format!(
                "rbd create --size {}G --image-feature layering,exclusive-lock {}/{}",
                vol.spec.size_gib, self.pool, vol.meta.id
            ),
        ))
    }

    async fn delete(&self, vol: &Volume) -> ApiResult<()> {
        Err(Self::todo(
            "delete",
            format!("rbd rm {}/{}", self.pool, vol.meta.id),
        ))
    }

    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef> {
        Err(Self::todo(
            "snapshot",
            format!(
                "rbd snap create {p}/{v}@{s} && rbd snap protect {p}/{v}@{s}",
                p = self.pool,
                v = vol.meta.id,
                s = snap.meta.id
            ),
        ))
    }

    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()> {
        Err(Self::todo(
            "delete_snapshot",
            format!(
                "rbd snap unprotect {p}/{v}@{s} && rbd snap rm {p}/{v}@{s}",
                p = self.pool,
                v = snap.spec.volume,
                s = snap.meta.id
            ),
        ))
    }

    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef> {
        Err(Self::todo(
            "restore",
            format!(
                "rbd clone {p}/{v}@{s} {p}/{n}",
                p = self.pool,
                v = snap.spec.volume,
                s = snap.meta.id,
                n = into.meta.id
            ),
        ))
    }

    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()> {
        Err(Self::todo(
            "extend",
            format!(
                "rbd resize --size {new_size_gib}G {}/{}",
                self.pool, vol.meta.id
            ),
        ))
    }

    async fn attach_info(&self, vol: &Volume, _host: &str) -> ApiResult<AttachInfo> {
        Err(Self::todo(
            "attach_info",
            format!("rbd map {}/{}", self.pool, vol.meta.id),
        ))
    }
}
