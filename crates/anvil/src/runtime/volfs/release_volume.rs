// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::detach_loop_idempotent::detach_loop_idempotent;
use super::io_err::io_err;
use super::read_sidecar::read_sidecar;
use super::unmount_idempotent::unmount_idempotent;
use super::volume_paths::volume_paths;
use pistack_types::ApiResult;
use pistack_types::Id;

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
