// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::block_source::block_source;
use super::canonical_fs::canonical_fs;
use super::command_path_ok::command_path_ok;
use super::detach_loop_idempotent::detach_loop_idempotent;
use super::disk_kind::disk_kind;
use super::disk_kind::DiskKind;
use super::filesystem_type::filesystem_type;
use super::losetup_find::losetup_find;
use super::make_filesystem::make_filesystem;
use super::mount_device::mount_device;
use super::prepare_mount_dir::prepare_mount_dir;
use super::release_volume::release_volume;
use super::rollback::rollback;
use super::volume_paths::volume_paths;
use super::write_sidecar::write_sidecar;
use crate::runtime::VolumeAttach;
use pistack_types::{ApiError, ApiResult};

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
