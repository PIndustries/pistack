// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Copy a mold image onto a new volume.
//!
//! Create checks the image before the volume row exists. The reconciler
//! stages the bytes (a local file, or a spool of the store's stream) and
//! the backend writes them while the volume stays `Creating`.

use pistack_store::StoreError;
use pistack_types::{ApiError, ApiResult, Id, Phase};
use std::path::{Path, PathBuf};

use crate::backend::gib_to_bytes;

pub(crate) struct CheckedImage {
    pub image: mold::Image,
    pub size_gib: u64,
}

/// A file `qemu-img` can read. `temporary` files are removed on drop.
pub(crate) struct StagedImage {
    pub path: PathBuf,
    pub format: String,
    temporary: bool,
}

impl Drop for StagedImage {
    fn drop(&mut self) {
        if self.temporary {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}

/// `requested_gib` of 0 takes the virtual size rounded up to the next GiB.
/// A known virtual size larger than the requested volume is rejected.
pub(crate) fn image_volume_size(requested_gib: u64, virtual_bytes: Option<u64>) -> ApiResult<u64> {
    match (requested_gib, virtual_bytes) {
        (0, None) => Err(ApiError::bad_request(
            "image has no virtual size; set spec.size_gib",
        )),
        (0, Some(bytes)) => Ok(bytes_to_gib(bytes)),
        (size, Some(bytes)) if bytes > gib_to_bytes(size) => Err(ApiError::unprocessable(format!(
            "image virtual size {bytes} bytes exceeds volume size {size} GiB"
        ))),
        (size, _) => Ok(size),
    }
}

fn bytes_to_gib(bytes: u64) -> u64 {
    const GIB: u64 = 1024 * 1024 * 1024;
    bytes.div_ceil(GIB).max(1)
}

/// Inspected size wins. A raw image with no probed size is the file length.
pub(crate) fn image_virtual_bytes(
    img: &mold::Image,
    insp: &mold::inspect::Inspection,
) -> Option<u64> {
    if let Some(size) = insp.virtual_size {
        return Some(size);
    }
    if let Some(size) = img.status.virtual_size_bytes {
        return Some(size);
    }
    if img.spec.format == mold::ImageFormat::Raw {
        return img.status.size_bytes;
    }
    None
}

/// Look the image up, require it ready, and reject backing files and a
/// volume that cannot hold the virtual disk.
pub(crate) async fn check_image(
    mold: &mold::Client,
    id: &Id,
    requested_gib: u64,
) -> ApiResult<CheckedImage> {
    let img = match mold.images().get(id) {
        Ok(img) => img,
        Err(StoreError::NotFound(_)) => {
            return Err(ApiError::not_found(format!("image {id} not found")))
        }
        Err(e) => return Err(e.into()),
    };
    if img.status.phase != Phase::Ready {
        return Err(ApiError::conflict(format!(
            "image {id} is {:?}, not ready",
            img.status.phase
        )));
    }
    if img.spec.format == mold::ImageFormat::Oci {
        return Err(ApiError::unprocessable(format!(
            "image {id} is an oci bundle and cannot seed a volume"
        )));
    }
    let insp = inspect_image(mold, &img).await?;
    let size_gib = image_volume_size(requested_gib, image_virtual_bytes(&img, &insp))?;
    Ok(CheckedImage {
        image: img,
        size_gib,
    })
}

async fn inspect_image(
    mold: &mold::Client,
    img: &mold::Image,
) -> ApiResult<mold::inspect::Inspection> {
    let local = local_image_file(mold, img).await;
    let data = if local.is_none() {
        Some(mold.open_image(img).await?)
    } else {
        None
    };
    let insp = mold::inspect::inspect(local.as_deref(), data, img.spec.format).await?;
    insp.reject_unsafe()?;
    mold::inspect::check_declared(img.spec.format, &insp)?;
    Ok(insp)
}

/// Absolute path of the blob when mold stored one on this host.
pub(crate) async fn local_image_file(mold: &mold::Client, img: &mold::Image) -> Option<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(path) = img.status.file_path.as_deref() {
        candidates.push(PathBuf::from(path));
    }
    if let Some(reference) = img.status.backing_ref.as_deref() {
        if let Some(path) = mold.blobs().local_path(reference) {
            candidates.push(path);
        }
    }
    candidates.into_iter().find(|path| is_file(path))
}

fn is_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|meta| meta.is_file())
        .unwrap_or(false)
}

/// Local blob path, or a temp file under `spool_dir` filled from the stream.
pub(crate) async fn stage_image(
    mold: &mold::Client,
    img: &mold::Image,
    spool_dir: &Path,
) -> ApiResult<StagedImage> {
    let format = img.spec.format.to_string();
    if let Some(path) = local_image_file(mold, img).await {
        return Ok(StagedImage {
            path,
            format,
            temporary: false,
        });
    }
    let mut data = mold.open_image(img).await?;
    tokio::fs::create_dir_all(spool_dir)
        .await
        .map_err(spool_io)?;
    let path = spool_dir.join(format!("{}-{}.img", img.meta.id, uuid::Uuid::now_v7()));
    if let Err(e) = spool_stream(&mut data, &path).await {
        let _ = tokio::fs::remove_file(&path).await;
        return Err(e);
    }
    Ok(StagedImage {
        path,
        format,
        temporary: true,
    })
}

async fn spool_stream(data: &mut mold::ImageData, dest: &Path) -> ApiResult<()> {
    let mut file = tokio::fs::File::create(dest).await.map_err(spool_io)?;
    tokio::io::copy(data, &mut file).await.map_err(spool_io)?;
    file.sync_all().await.map_err(spool_io)?;
    Ok(())
}

fn spool_io(err: std::io::Error) -> ApiError {
    ApiError::internal(format!("image spool: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * 1024 * 1024;

    #[test]
    fn omitted_size_rounds_up_and_a_short_volume_is_rejected() {
        assert_eq!(image_volume_size(0, Some(1)).unwrap(), 1);
        assert_eq!(image_volume_size(0, Some(GIB)).unwrap(), 1);
        assert_eq!(image_volume_size(0, Some(GIB + 1)).unwrap(), 2);
        assert_eq!(image_volume_size(2, Some(GIB + 1)).unwrap(), 2);
        assert_eq!(image_volume_size(4, None).unwrap(), 4);
        let err = image_volume_size(1, Some(GIB + 1)).unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unprocessable);
        assert!(err.message.contains("exceeds"), "{err}");
        let err = image_volume_size(0, None).unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);
    }

    use crate::client::{Client, Create};
    use crate::types::{VolumeSource, VolumeSpec};
    use async_trait::async_trait;
    use pistack_types::{kind, ApiError, Meta, Phase};
    use std::path::{Path, PathBuf};
    use std::time::Duration;

    fn qcow2(path: &Path, virtual_bytes: u64, backing: Option<&Path>) {
        if mold::inspect::qemu_img_available() {
            let mut cmd = std::process::Command::new("qemu-img");
            cmd.arg("create").arg("-f").arg("qcow2");
            if let Some(base) = backing {
                cmd.arg("-b").arg(base).arg("-F").arg("raw");
            }
            cmd.arg(path).arg(virtual_bytes.to_string());
            let out = cmd.output().expect("qemu-img create");
            assert!(
                out.status.success(),
                "qemu-img create: {}",
                String::from_utf8_lossy(&out.stderr)
            );
            return;
        }
        let mut header = vec![0u8; 512];
        header[..4].copy_from_slice(b"QFI\xfb");
        header[4..8].copy_from_slice(&3u32.to_be_bytes());
        header[20..24].copy_from_slice(&16u32.to_be_bytes());
        header[24..32].copy_from_slice(&virtual_bytes.to_be_bytes());
        if let Some(base) = backing {
            let name = base.to_string_lossy();
            let bytes = name.as_bytes();
            let off = 104usize;
            header[8..16].copy_from_slice(&(off as u64).to_be_bytes());
            header[16..20].copy_from_slice(&(bytes.len() as u32).to_be_bytes());
            let end = off + bytes.len();
            if header.len() < end {
                header.resize(end, 0);
            }
            header[off..end].copy_from_slice(bytes);
        }
        std::fs::write(path, header).unwrap();
    }

    fn image_of(
        name: &str,
        format: mold::ImageFormat,
        phase: Phase,
        path: Option<&Path>,
        virtual_size: Option<u64>,
    ) -> mold::Image {
        mold::Image {
            meta: Meta::new(mold::KIND_IMAGE, name, None),
            spec: mold::ImageSpec {
                format,
                os: None,
                arch: "x86_64".into(),
                min_disk_gib: 0,
                min_ram_mib: 0,
                visibility: mold::Visibility::Private,
                source_url: None,
            },
            status: mold::ImageStatus {
                phase,
                size_bytes: virtual_size,
                sha256: None,
                backing_ref: Some("sha256:image".into()),
                file_path: path.map(|p| p.display().to_string()),
                virtual_size_bytes: virtual_size,
                conditions: Vec::new(),
            },
        }
    }

    fn client(dir: &Path) -> Client {
        let client = Client::local(dir).unwrap();
        client.seed_defaults().unwrap();
        client
    }

    fn save(client: &Client, image: mold::Image) -> mold::Image {
        client.mold().images().create(image).unwrap()
    }

    async fn create_from(
        client: &Client,
        name: &str,
        size_gib: u64,
        image: &Id,
    ) -> ApiResult<crate::types::Volume> {
        client
            .create_volume(Create {
                name: name.into(),
                description: String::new(),
                labels: Default::default(),
                annotations: Default::default(),
                project: None,
                spec: VolumeSpec {
                    size_gib,
                    volume_type: "standard".into(),
                    source: VolumeSource::Image(image.clone()),
                    bootable: false,
                    encrypted: false,
                },
            })
            .await
    }

    fn assert_no_volume(client: &Client) {
        assert!(client.list_volumes(None).unwrap().is_empty());
    }

    #[test]
    fn omitted_size_gib_deserializes_as_zero() {
        let spec: VolumeSpec = serde_json::from_value(serde_json::json!({
            "source": {
                "kind": "image",
                "id": "img-0199a5e0-1111-7000-8000-0000000000ab"
            }
        }))
        .unwrap();
        assert_eq!(spec.size_gib, 0);
        assert!(matches!(spec.source, VolumeSource::Image(_)));
    }

    #[tokio::test]
    async fn missing_image_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let id = Id::new(kind::IMAGE);
        let err = create_from(&client, "boot", 1, &id).await.unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::NotFound);
        assert!(err.message.contains("not found"), "{err}");
        assert_no_volume(&client);
    }

    #[tokio::test]
    async fn image_must_be_ready() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let image = save(
            &client,
            image_of("ubuntu", mold::ImageFormat::Raw, Phase::Pending, None, None),
        );
        let err = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Conflict);
        assert!(err.message.contains("not ready"), "{err}");
        assert_no_volume(&client);
    }

    #[tokio::test]
    async fn oci_bundle_cannot_seed_a_volume() {
        let dir = tempfile::tempdir().unwrap();
        let client = client(dir.path());
        let image = save(
            &client,
            image_of("app", mold::ImageFormat::Oci, Phase::Ready, None, None),
        );
        let err = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unprocessable);
        assert!(err.message.contains("oci bundle"), "{err}");
        assert_no_volume(&client);
    }

    #[tokio::test]
    async fn volume_smaller_than_the_image_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("big.qcow2");
        qcow2(&disk, 2 * GIB, None);
        let client = client(dir.path());
        let image = save(
            &client,
            image_of(
                "ubuntu",
                mold::ImageFormat::Qcow2,
                Phase::Ready,
                Some(&disk),
                Some(2 * GIB),
            ),
        );
        let err = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unprocessable);
        assert!(err.message.contains("exceeds"), "{err}");
        assert_no_volume(&client);

        let exact = dir.path().join("exact.qcow2");
        qcow2(&exact, GIB, None);
        let fit = save(
            &client,
            image_of(
                "fit",
                mold::ImageFormat::Qcow2,
                Phase::Ready,
                Some(&exact),
                Some(GIB),
            ),
        );
        let vol = create_from(&client, "fits", 1, &fit.meta.id).await.unwrap();
        assert_eq!(vol.spec.size_gib, 1);
        assert!(matches!(vol.spec.source, VolumeSource::Image(id) if id == fit.meta.id));
    }

    #[tokio::test]
    async fn backing_file_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let base = dir.path().join("base.raw");
        std::fs::write(&base, vec![0u8; 64]).unwrap();
        let disk = dir.path().join("overlay.qcow2");
        qcow2(&disk, GIB, Some(&base));
        let client = client(dir.path());
        let image = save(
            &client,
            image_of(
                "overlay",
                mold::ImageFormat::Qcow2,
                Phase::Ready,
                Some(&disk),
                Some(GIB),
            ),
        );
        let err = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::Unprocessable);
        assert!(err.message.contains("backing file"), "{err}");
        assert_no_volume(&client);
    }

    #[tokio::test]
    async fn omitted_size_uses_the_image_virtual_size() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("disk.qcow2");
        qcow2(&disk, GIB + 1, None);
        let client = client(dir.path());
        let image = save(
            &client,
            image_of(
                "ubuntu",
                mold::ImageFormat::Qcow2,
                Phase::Ready,
                Some(&disk),
                Some(GIB + 1),
            ),
        );
        let vol = create_from(&client, "boot", 0, &image.meta.id)
            .await
            .unwrap();
        assert_eq!(vol.spec.size_gib, 2);
        assert_eq!(vol.status.phase, Phase::Pending);

        let err = client
            .create_volume(Create {
                name: "blank".into(),
                description: String::new(),
                labels: Default::default(),
                annotations: Default::default(),
                project: None,
                spec: VolumeSpec {
                    size_gib: 0,
                    volume_type: "standard".into(),
                    source: VolumeSource::None,
                    bootable: false,
                    encrypted: false,
                },
            })
            .await
            .unwrap_err();
        assert_eq!(err.code, pistack_types::error::ErrorCode::BadRequest);
        assert!(err.message.contains("at least 1"), "{err}");
    }

    struct MemBlobs {
        bytes: Vec<u8>,
    }

    #[async_trait]
    impl mold::ImageStore for MemBlobs {
        fn name(&self) -> &'static str {
            "mem"
        }
        async fn put(&self, _data: mold::ImageData) -> ApiResult<mold::Stored> {
            Err(ApiError::internal("unused"))
        }
        async fn open(&self, _reference: &str) -> ApiResult<mold::ImageData> {
            let data: mold::ImageData = Box::pin(std::io::Cursor::new(self.bytes.clone()));
            Ok(data)
        }
        async fn delete(&self, _reference: &str) -> ApiResult<()> {
            Ok(())
        }
        fn local_path(&self, _reference: &str) -> Option<PathBuf> {
            None
        }
    }

    fn raw_image(name: &str, bytes: &[u8], path: Option<&Path>) -> mold::Image {
        let mut image = image_of(
            name,
            mold::ImageFormat::Raw,
            Phase::Ready,
            path,
            Some(bytes.len() as u64),
        );
        image.status.size_bytes = Some(bytes.len() as u64);
        if path.is_none() {
            image.status.backing_ref = Some("mem:seed".into());
            image.status.file_path = None;
        }
        image
    }

    #[tokio::test]
    async fn copies_a_local_image_into_the_volume() {
        let _qemu = crate::backend::QEMU_IMG_LOCK.lock().await;
        if !mold::inspect::qemu_img_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = b"PISTACK-IMAGE-SEED".to_vec();
        bytes.resize(4096, 0);
        let disk = dir.path().join("seed.raw");
        std::fs::write(&disk, &bytes).unwrap();
        let client = client(dir.path());
        let image = save(&client, raw_image("ubuntu", &bytes, Some(&disk)));
        let created = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap();
        let vol = client
            .reconcile_volume(&created.meta.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vol.status.phase, Phase::Ready, "{vol:?}");
        assert!(matches!(vol.spec.source, VolumeSource::Image(id) if id == image.meta.id));
        let written = vol
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "ImageWritten")
            .expect("ImageWritten");
        assert!(written.status);
        assert_eq!(written.reason, "copied");
        assert_eq!(written.message, image.meta.id.to_string());
        let path = dir.path().join(format!("{}.raw", vol.meta.id));
        assert_eq!(std::fs::metadata(&path).unwrap().len(), GIB);
        let mut got = vec![0u8; bytes.len()];
        use std::io::Read;
        std::fs::File::open(&path)
            .unwrap()
            .read_exact(&mut got)
            .unwrap();
        assert_eq!(got, bytes);
        assert_spool_empty(dir.path());
    }

    #[tokio::test]
    async fn failed_copy_marks_error_and_removes_the_volume() {
        let _qemu = crate::backend::QEMU_IMG_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"seed-bytes".to_vec();
        let disk = dir.path().join("seed.raw");
        std::fs::write(&disk, &bytes).unwrap();
        let script = dir.path().join("qemu-img");
        std::fs::write(&script, "#!/bin/sh\necho convert refused >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(crate::backend::ENV_QEMU_IMG, &script);
        struct Reset;
        impl Drop for Reset {
            fn drop(&mut self) {
                std::env::remove_var(crate::backend::ENV_QEMU_IMG);
            }
        }
        let _reset = Reset;

        let client = client(dir.path());
        let image = save(&client, raw_image("ubuntu", &bytes, Some(&disk)));
        let created = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap();
        let vol = client
            .reconcile_volume(&created.meta.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vol.status.phase, Phase::Error, "{vol:?}");
        assert!(vol.status.backend_ref.is_none(), "{vol:?}");
        let message = vol
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Provisioned")
            .map(|c| c.message.as_str())
            .unwrap_or("");
        assert!(message.contains("copying image"), "{message}");
        assert!(message.contains("convert refused"), "{message}");
        assert!(matches!(vol.spec.source, VolumeSource::Image(id) if id == image.meta.id));
        assert!(!dir.path().join(format!("{}.raw", vol.meta.id)).exists());
    }

    #[tokio::test]
    async fn volume_stays_creating_while_the_image_is_copied() {
        let _qemu = crate::backend::QEMU_IMG_LOCK.lock().await;
        let dir = tempfile::tempdir().unwrap();
        let bytes = b"seed-bytes".to_vec();
        let disk = dir.path().join("seed.raw");
        std::fs::write(&disk, &bytes).unwrap();
        let started = dir.path().join("started");
        let go = dir.path().join("go");
        let script = dir.path().join("qemu-img");
        let started_q = started.display().to_string().replace('\'', "'\\''");
        let go_q = go.display().to_string().replace('\'', "'\\''");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\ntouch '{started_q}'\ni=0\nwhile [ ! -f '{go_q}' ]; do\n  i=$((i+1))\n  if [ \"$i\" -gt 100 ]; then echo timed out >&2; exit 1; fi\n  sleep 0.1\ndone\nexit 0\n"
            ),
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::env::set_var(crate::backend::ENV_QEMU_IMG, &script);
        let go_drop = go.clone();
        struct Reset(PathBuf);
        impl Drop for Reset {
            fn drop(&mut self) {
                let _ = std::fs::write(&self.0, b"1");
                std::env::remove_var(crate::backend::ENV_QEMU_IMG);
            }
        }
        let _reset = Reset(go_drop);

        let client = client(dir.path());
        let image = save(&client, raw_image("ubuntu", &bytes, Some(&disk)));
        let created = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap();
        let worker = client.clone();
        let vol_id = created.meta.id.clone();
        let handle = tokio::spawn(async move { worker.reconcile_volume(&vol_id).await });
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        while !started.exists() {
            if handle.is_finished() {
                let err = handle.await.expect("reconcile task");
                panic!("reconcile finished before convert: {err:?}");
            }
            if std::time::Instant::now() >= deadline {
                panic!("convert did not start");
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let mid = client
            .get_volume(None, &created.meta.id.to_string())
            .unwrap();
        assert_eq!(mid.status.phase, Phase::Creating, "{mid:?}");
        std::fs::write(&go, b"1").unwrap();
        let vol = tokio::time::timeout(Duration::from_secs(5), handle)
            .await
            .expect("reconcile timed out")
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(vol.status.phase, Phase::Ready, "{vol:?}");
    }

    #[tokio::test]
    async fn streams_an_image_through_a_spool_file() {
        let _qemu = crate::backend::QEMU_IMG_LOCK.lock().await;
        if !mold::inspect::qemu_img_available() {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = b"PISTACK-STREAM-SEED".to_vec();
        bytes.resize(4096, 0);
        let client = client(dir.path()).with_images(std::sync::Arc::new(MemBlobs {
            bytes: bytes.clone(),
        }));
        let image = save(&client, raw_image("ubuntu", &bytes, None));
        let created = create_from(&client, "boot", 1, &image.meta.id)
            .await
            .unwrap();
        let vol = client
            .reconcile_volume(&created.meta.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(vol.status.phase, Phase::Ready, "{vol:?}");
        assert!(matches!(vol.spec.source, VolumeSource::Image(id) if id == image.meta.id));
        let path = dir.path().join(format!("{}.raw", vol.meta.id));
        let mut got = vec![0u8; bytes.len()];
        use std::io::Read;
        std::fs::File::open(&path)
            .unwrap()
            .read_exact(&mut got)
            .unwrap();
        assert_eq!(got, bytes);
        assert_spool_empty(dir.path());
    }

    fn assert_spool_empty(dir: &Path) {
        let spool = dir.join("spool");
        if !spool.exists() {
            return;
        }
        let left: Vec<_> = std::fs::read_dir(&spool)
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert!(left.is_empty(), "spool left {left:?}");
    }
}
