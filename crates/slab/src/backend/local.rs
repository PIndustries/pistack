// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Sparse raw files under one directory. The development backend, and good
//! enough for a single host: `anvil` attaches the file directly.
//!
//! Layout: `<dir>/<vol-id>.raw`, `<dir>/snapshots/<snap-id>.raw`.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;

use super::{gib_to_bytes, AttachInfo, AttachTarget, BackendRef, BlockBackend};
use crate::types::{Snapshot, Volume, VolumeSource};

pub struct LocalFileBackend {
    dir: PathBuf,
}

impl LocalFileBackend {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn volume_path(&self, vol: &Volume) -> PathBuf {
        self.dir.join(format!("{}.raw", vol.meta.id))
    }

    pub fn snapshot_path(&self, snap: &Snapshot) -> PathBuf {
        self.dir
            .join("snapshots")
            .join(format!("{}.raw", snap.meta.id))
    }

    /// Path recorded in `backend_ref`, falling back to the canonical one.
    fn vol_ref_path(&self, vol: &Volume) -> PathBuf {
        vol.status
            .backend_ref
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.volume_path(vol))
    }

    fn snap_ref_path(&self, snap: &Snapshot) -> PathBuf {
        snap.status
            .backend_ref
            .as_deref()
            .map(PathBuf::from)
            .unwrap_or_else(|| self.snapshot_path(snap))
    }

    async fn ensure_dirs(&self) -> ApiResult<()> {
        fs::create_dir_all(self.dir.join("snapshots"))
            .await
            .map_err(io_err)?;
        Ok(())
    }

    async fn remove_if_exists(path: &Path) -> ApiResult<()> {
        match fs::remove_file(path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }

    /// Copy `from` to `to`, then make sure `to` is at least `size` bytes.
    async fn copy_and_size(from: &Path, to: &Path, size: u64) -> ApiResult<()> {
        if !fs::try_exists(from).await.map_err(io_err)? {
            return Err(ApiError::not_found(format!(
                "source {} does not exist",
                from.display()
            )));
        }
        let (from, to) = (from.to_path_buf(), to.to_path_buf());
        tokio::task::spawn_blocking(move || sparse_copy(&from, &to, size))
            .await
            .map_err(|e| ApiError::internal(format!("local backend: copy task: {e}")))?
            .map_err(io_err)
    }
}

/// Copy preserving holes: runs of zero blocks are skipped with a seek so a
/// 100 GiB volume that holds 2 GiB of data copies in seconds and stays
/// sparse. Blocking; call from `spawn_blocking`.
fn sparse_copy(from: &Path, to: &Path, min_size: u64) -> io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};
    const CHUNK: usize = 1024 * 1024;
    let mut src = std::fs::File::open(from)?;
    let mut dst = std::fs::File::create(to)?;
    let src_len = src.metadata()?.len();
    let mut buf = vec![0u8; CHUNK];
    // Slice equality is a memcmp, fast even unoptimised; a byte loop is not.
    let zeros = vec![0u8; CHUNK];
    let mut pos: u64 = 0;
    loop {
        let n = src.read(&mut buf)?;
        if n == 0 {
            break;
        }
        if buf[..n] == zeros[..n] {
            dst.seek(SeekFrom::Current(n as i64))?;
        } else {
            dst.write_all(&buf[..n])?;
        }
        pos += n as u64;
    }
    dst.set_len(pos.max(src_len).max(min_size))?;
    dst.sync_all()
}

fn io_err(e: io::Error) -> ApiError {
    ApiError::internal(format!("local backend: {e}"))
}

#[async_trait]
impl BlockBackend for LocalFileBackend {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn create(&self, vol: &Volume) -> ApiResult<BackendRef> {
        self.ensure_dirs().await?;
        let path = self.volume_path(vol);
        let size = gib_to_bytes(vol.spec.size_gib);
        match &vol.spec.source {
            VolumeSource::None | VolumeSource::Image(_) => {
                // Image population is the reconciler's job (it streams from
                // mold); the backend only provides the blank device.
                let f = fs::File::create(&path).await.map_err(io_err)?;
                f.set_len(size).await.map_err(io_err)?;
            }
            VolumeSource::Volume(src) => {
                let from = self.dir.join(format!("{src}.raw"));
                Self::copy_and_size(&from, &path, size).await?;
            }
            VolumeSource::Snapshot(src) => {
                let from = self.dir.join("snapshots").join(format!("{src}.raw"));
                Self::copy_and_size(&from, &path, size).await?;
            }
        }
        Ok(BackendRef::new(path.to_string_lossy().into_owned()))
    }

    async fn delete(&self, vol: &Volume) -> ApiResult<()> {
        Self::remove_if_exists(&self.vol_ref_path(vol)).await
    }

    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef> {
        self.ensure_dirs().await?;
        let from = self.vol_ref_path(vol);
        let to = self.snapshot_path(snap);
        Self::copy_and_size(&from, &to, 0).await?;
        Ok(BackendRef::new(to.to_string_lossy().into_owned()))
    }

    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()> {
        Self::remove_if_exists(&self.snap_ref_path(snap)).await
    }

    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef> {
        self.ensure_dirs().await?;
        let from = self.snap_ref_path(snap);
        let to = self.volume_path(into);
        Self::copy_and_size(&from, &to, gib_to_bytes(into.spec.size_gib)).await?;
        Ok(BackendRef::new(to.to_string_lossy().into_owned()))
    }

    async fn extend(&self, vol: &Volume, new_size_gib: u64) -> ApiResult<()> {
        let path = self.vol_ref_path(vol);
        let f = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(io_err)?;
        let cur = f.metadata().await.map_err(io_err)?.len();
        let want = gib_to_bytes(new_size_gib);
        if want < cur {
            return Err(ApiError::unprocessable("volumes cannot shrink"));
        }
        f.set_len(want).await.map_err(io_err)?;
        Ok(())
    }

    async fn attach_info(&self, vol: &Volume, _host: &str) -> ApiResult<AttachInfo> {
        let path = self.vol_ref_path(vol);
        let mut info = AttachInfo::new(AttachTarget::Local {
            path: path.to_string_lossy().into_owned(),
        });
        info.device = vol.status.device.clone();
        Ok(info)
    }
}
