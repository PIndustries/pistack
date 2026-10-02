// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! LVM thin-pool backend. Stub: every method returns `not_implemented`.
//!
//! When wired, the backend runs these on the storage host (via `anvil`
//! over RPC, never locally on the control plane):
//!
//! | operation       | command |
//! | --------------- | ------- |
//! | create          | `lvcreate --thin --virtualsize {size_gib}G --name {vol_id} {vg}/{pool}` |
//! | delete          | `lvremove --force {vg}/{vol_id}` |
//! | snapshot        | `lvcreate --snapshot --name {snap_id} {vg}/{vol_id}` (thin snapshots need no size) |
//! | delete_snapshot | `lvremove --force {vg}/{snap_id}` |
//! | restore         | `lvcreate --snapshot --name {new_vol_id} {vg}/{snap_id}` then `lvchange -ay -K {vg}/{new_vol_id}` |
//! | extend          | `lvextend --size {new_size_gib}G {vg}/{vol_id}` |
//! | attach_info     | same host: `Local { path: /dev/{vg}/{vol_id} }`; remote host: export with `targetcli` and return `Iscsi { portal, iqn, lun }` |
//!
//! Parameters come from `VolumeTypeSpec::params`: `vg` (required),
//! `pool` (thin pool, default `pistack-thin`), `iscsi_portal`.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};

use super::{AttachInfo, BackendRef, BlockBackend};
use crate::types::{Snapshot, Volume};

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

    fn todo(op: &str, cmd: String) -> ApiError {
        ApiError::not_implemented(format!("lvm backend: {op} not implemented"))
            .with_detail("command", cmd)
    }
}

#[async_trait]
impl BlockBackend for LvmBackend {
    fn name(&self) -> &'static str {
        "lvm"
    }

    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef> {
        Err(Self::todo(
            "create",
            format!(
                "lvcreate --thin --virtualsize {}G --name {} {}/{}",
                vol.spec.size_gib, vol.meta.id, self.vg, self.pool
            ),
        ))
    }

    async fn delete(&self, vol: &Volume) -> ApiResult<()> {
        Err(Self::todo(
            "delete",
            format!("lvremove --force {}/{}", self.vg, vol.meta.id),
        ))
    }

    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef> {
        Err(Self::todo(
            "snapshot",
            format!(
                "lvcreate --snapshot --name {} {}/{}",
                snap.meta.id, self.vg, vol.meta.id
            ),
        ))
    }

    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()> {
        Err(Self::todo(
            "delete_snapshot",
            format!("lvremove --force {}/{}", self.vg, snap.meta.id),
        ))
    }

    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef> {
        Err(Self::todo(
            "restore",
            format!(
                "lvcreate --snapshot --name {} {}/{} && lvchange -ay -K {}/{}",
                into.meta.id, self.vg, snap.meta.id, self.vg, into.meta.id
            ),
        ))
    }

    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()> {
        Err(Self::todo(
            "extend",
            format!(
                "lvextend --size {new_size_gib}G {}/{}",
                self.vg, vol.meta.id
            ),
        ))
    }

    async fn attach_info(&self, vol: &Volume, host: &str) -> ApiResult<AttachInfo> {
        Err(Self::todo(
            "attach_info",
            format!(
                "on {host}: Local {{ path: /dev/{}/{} }} or targetcli iSCSI export",
                self.vg, vol.meta.id
            ),
        ))
    }
}
