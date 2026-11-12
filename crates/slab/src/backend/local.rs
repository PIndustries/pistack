// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Sparse raw files under one directory. The development backend, and good
//! enough for a single host: `anvil` attaches the file directly. The path
//! means something only on the node that holds the file, so the client
//! checks the host before it hands the path out.
//!
//! Layout: `<dir>/<vol-id>.raw`, `<dir>/snapshots/<snap-id>.raw`.
//!
//! An image is copied with `qemu-img convert -O raw -f <fmt> -n <src>
//! <dir>/<vol-id>.raw`. `-n` keeps the sparse file at `size_gib`.

use async_trait::async_trait;
use pistack_types::error::ErrorCode;
use pistack_types::{ApiError, ApiResult};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::AsyncSeekExt;

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

    /// `qemu-img convert -O raw -f <format> -n <src> <volume file>`.
    fn image_convert_args(
        &self,
        vol: &Volume,
        src: &std::path::Path,
        format: &str,
    ) -> ApiResult<Vec<String>> {
        let dest = self.vol_ref_path(vol).to_string_lossy().into_owned();
        Ok(super::qemu_img_convert_argv(format, src, &dest))
    }

    async fn prepare_raw_file(&self, vol: &Volume) -> ApiResult<()> {
        self.ensure_dirs().await?;
        let path = self.vol_ref_path(vol);
        if fs::try_exists(&path).await.map_err(io_err)? {
            return Ok(());
        }
        let file = fs::File::create(&path).await.map_err(io_err)?;
        file.set_len(gib_to_bytes(vol.spec.size_gib))
            .await
            .map_err(io_err)?;
        Ok(())
    }

    async fn ensure_size(&self, vol: &Volume) -> ApiResult<()> {
        let path = self.vol_ref_path(vol);
        let file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(io_err)?;
        let cur = file.metadata().await.map_err(io_err)?.len();
        let want = gib_to_bytes(vol.spec.size_gib);
        if cur < want {
            file.set_len(want).await.map_err(io_err)?;
        }
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
                // Blank file of the full size. Image bytes are written into
                // it afterwards (`qemu-img convert -n`) so it is not shrunk
                // to the image.
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

    async fn write_image(
        &self,
        vol: &Volume,
        src: &std::path::Path,
        format: &str,
    ) -> ApiResult<()> {
        self.prepare_raw_file(vol).await?;
        let args = self.image_convert_args(vol, src, format)?;
        super::run_qemu_img(src, &args).await?;
        self.ensure_size(vol).await
    }

    async fn delete(&self, vol: &Volume) -> ApiResult<()> {
        self.remove_owned(&self.vol_ref_path(vol)).await
    }

    async fn snapshot(&self, vol: &Volume, snap: &Snapshot) -> ApiResult<BackendRef> {
        self.ensure_dirs().await?;
        let from = self.vol_ref_path(vol);
        let to = self.snapshot_path(snap);
        Self::copy_and_size(&from, &to, 0).await?;
        Ok(BackendRef::new(to.to_string_lossy().into_owned()))
    }

    async fn delete_snapshot(&self, snap: &Snapshot) -> ApiResult<()> {
        self.remove_owned(&self.snap_ref_path(snap)).await
    }

    async fn restore(&self, snap: &Snapshot, into: &Volume) -> ApiResult<BackendRef> {
        self.ensure_dirs().await?;
        let from = self.snap_ref_path(snap);
        let to = self.volume_path(into);
        Self::copy_and_size(&from, &to, gib_to_bytes(into.spec.size_gib)).await?;
        Ok(BackendRef::new(to.to_string_lossy().into_owned()))
    }

    async fn clone_volume(&self, src: &Volume, into: &Volume) -> ApiResult<BackendRef> {
        self.ensure_dirs().await?;
        let from = self.vol_ref_path(src);
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

    async fn bytes_path(&self, vol: &Volume) -> ApiResult<PathBuf> {
        self.confine(&self.vol_ref_path(vol)).await
    }

    async fn open_backup(&self, vol: &Volume) -> ApiResult<super::BackupStream> {
        let path = self.bytes_path(vol).await?;
        let file = fs::File::open(&path).await.map_err(io_err)?;
        Ok(super::BackupStream::file(file))
    }

    async fn open_restore(&self, vol: &Volume) -> ApiResult<super::RestoreStream> {
        let path = self.bytes_path(vol).await?;
        let mut file = fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .await
            .map_err(io_err)?;
        file.seek(io::SeekFrom::Start(0)).await.map_err(io_err)?;
        Ok(super::RestoreStream::file(file))
    }
}

impl LocalFileBackend {
    /// The volume file must stay inside this backend's directory.
    async fn confine(&self, path: &Path) -> ApiResult<PathBuf> {
        let dir = match fs::canonicalize(&self.dir).await {
            Ok(dir) => dir,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                return Err(ApiError::unavailable(
                    "local volume directory is not available",
                ));
            }
            Err(e) => return Err(io_err(e)),
        };
        match fs::canonicalize(path).await {
            Ok(canon) => {
                if !crate::path_inside(&dir, &canon) {
                    return Err(ApiError::bad_request(
                        "volume path escapes the slab directory",
                    ));
                }
                Ok(canon)
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let parent = path.parent().ok_or_else(|| {
                    ApiError::bad_request("volume path escapes the slab directory")
                })?;
                let parent = fs::canonicalize(parent).await.map_err(io_err)?;
                if !crate::path_inside(&dir, &parent) {
                    return Err(ApiError::bad_request(
                        "volume path escapes the slab directory",
                    ));
                }
                let name = path.file_name().ok_or_else(|| {
                    ApiError::bad_request("volume path escapes the slab directory")
                })?;
                Ok(parent.join(name))
            }
            Err(e) => Err(io_err(e)),
        }
    }

    /// Remove `path` only when it stays inside this backend's directory.
    /// A missing directory or a missing file is success. An outside path
    /// is refused and left in place.
    async fn remove_owned(&self, path: &Path) -> ApiResult<()> {
        match self.confine(path).await {
            Ok(path) => Self::remove_if_exists(&path).await,
            Err(e) if e.code == ErrorCode::Unavailable => Ok(()),
            Err(e) if e.code == ErrorCode::BadRequest => Err(e),
            Err(_) if self.lexical_inside(path) => Self::remove_if_exists(path).await,
            Err(e) => Err(e),
        }
    }

    /// Component-wise check used when the file is not there to canonicalize.
    /// Both the configured directory and its canonical form count, so a
    /// `/var` path and a `/private/var` path still agree.
    fn lexical_inside(&self, path: &Path) -> bool {
        let mut roots = vec![self.dir.clone()];
        if let Ok(canon) = std::fs::canonicalize(&self.dir) {
            roots.push(canon);
        }
        roots.into_iter().any(|root| path_under(&root, path))
    }
}

fn path_under(root: &Path, path: &Path) -> bool {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };
    crate::path_inside(&normalize(root), &normalize(&abs))
}

fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in path.components() {
        match c {
            std::path::Component::ParentDir => {
                out.pop();
            }
            std::path::Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{VolumeSpec, VolumeStatus};
    use pistack_types::{kind, Meta};
    use std::os::unix::fs::PermissionsExt;

    const GIB: u64 = 1024 * 1024 * 1024;

    fn volume(size: u64) -> Volume {
        Volume {
            meta: Meta::new(kind::VOLUME, "data", None),
            spec: VolumeSpec {
                size_gib: size,
                volume_type: "standard".into(),
                source: VolumeSource::Image(pistack_types::Id::new(kind::IMAGE)),
                bootable: false,
                encrypted: false,
            },
            status: VolumeStatus::default(),
        }
    }

    #[tokio::test]
    async fn image_convert_command_targets_the_volume_file() {
        let _qemu = super::super::QEMU_IMG_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let log = dir.path().join("argv.log");
        let script = dir.path().join("qemu-img");
        let quoted = log.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{quoted}'\n\
                 dest=\nfor a in \"$@\"; do dest=$a; done\nprintf 'x' > \"$dest\"\nexit 0\n"
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(super::super::ENV_QEMU_IMG, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(super::super::ENV_QEMU_IMG);
            }
        }
        let _reset = Reset;

        let backend = LocalFileBackend::new(dir.path());
        let vol = volume(1);
        let src = dir.path().join("seed.qcow2");
        std::fs::write(&src, b"seed").unwrap();
        let dest = backend.volume_path(&vol);
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
                dest.to_string_lossy().into_owned(),
            ]
        );

        backend.write_image(&vol, &src, "qcow2").await.unwrap();
        let logged = std::fs::read_to_string(&log).unwrap();
        assert_eq!(
            logged,
            args.iter().map(|a| format!("{a}\n")).collect::<String>()
        );
        assert_eq!(std::fs::metadata(&dest).unwrap().len(), GIB);

        std::env::set_var(
            super::super::ENV_QEMU_IMG,
            dir.path().join("missing-qemu-img"),
        );
        let err = backend.write_image(&vol, &src, "qcow2").await.unwrap_err();
        assert_eq!(
            err.code,
            pistack_types::error::ErrorCode::Unavailable,
            "{err}"
        );
    }
}
