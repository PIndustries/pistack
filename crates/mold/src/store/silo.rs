// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Image blobs stored through the silo object API.
//!
//! The bucket name defaults to `pistack-images`. Each blob is one object
//! whose key is `sha256/<hex>`. Bytes land in that bucket's directory
//! under the silo data root. The reference mold stores is `sha256:<hex>`.
//!
//! `put` reads the whole stream, then calls `silo::Client::put_object`.
//! The silo client accepts a byte slice, capped at 1 GiB. `open` is
//! `get_object`. `delete` is `delete_object` and treats a missing object
//! as success.

use async_trait::async_trait;
use pistack_types::{error::ErrorCode, ApiError, ApiResult, Meta, Spec};
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tokio::io::AsyncReadExt;

use super::{hex, ImageData, ImageStore, Stored};

/// Bucket used when the caller passes an empty name.
pub const DEFAULT_BUCKET: &str = "pistack-images";

/// Reference prefix mold already uses for content-addressed blobs.
const REF_PREFIX: &str = "sha256:";

pub struct SiloStore {
    pub bucket: String,
    client: ::silo::Client,
}

impl SiloStore {
    /// Store image bytes in `bucket` through `client`. An empty name
    /// selects [`DEFAULT_BUCKET`]. The bucket is created on the first put.
    pub fn new(client: ::silo::Client, bucket: impl Into<String>) -> Self {
        let bucket = bucket.into();
        let bucket = if bucket.is_empty() {
            DEFAULT_BUCKET.to_owned()
        } else {
            bucket
        };
        Self { bucket, client }
    }

    fn ensure_bucket(&self) -> ApiResult<pistack_types::Id> {
        match self.client.get_bucket(None, &self.bucket) {
            Ok(bucket) => Ok(bucket.meta.id),
            Err(err) if err.code == ErrorCode::NotFound => self.create_bucket(),
            Err(err) => Err(err),
        }
    }

    fn create_bucket(&self) -> ApiResult<pistack_types::Id> {
        let meta = Meta::new(::silo::BucketSpec::KIND, self.bucket.clone(), None);
        match self
            .client
            .create_bucket(meta, ::silo::BucketSpec { versioning: false })
        {
            Ok(bucket) => Ok(bucket.meta.id),
            Err(err) if err.code == ErrorCode::Conflict => {
                Ok(self.client.get_bucket(None, &self.bucket)?.meta.id)
            }
            Err(err) => Err(err),
        }
    }
}

fn object_key(reference: &str) -> ApiResult<String> {
    let hexd = reference
        .strip_prefix(REF_PREFIX)
        .or_else(|| reference.strip_prefix("sha256/"))
        .ok_or_else(|| ApiError::bad_request(format!("malformed image reference '{reference}'")))?;
    if hexd.len() != 64 || !hexd.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err(ApiError::bad_request(format!(
            "malformed image reference '{reference}'"
        )));
    }
    Ok(format!("sha256/{}", hexd.to_ascii_lowercase()))
}

fn missing(err: &ApiError) -> bool {
    err.code == ErrorCode::NotFound
}

/// Read `data` into one buffer. The silo put takes a byte slice and refuses
/// anything larger than [`silo::MAX_OBJECT_BYTES`].
async fn read_blob(data: &mut ImageData) -> ApiResult<(Vec<u8>, String, u64)> {
    let max = ::silo::MAX_OBJECT_BYTES as u64;
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 256 * 1024];
    let mut out = Vec::new();
    let mut size: u64 = 0;
    loop {
        let n = data
            .read(&mut buf)
            .await
            .map_err(|e| ApiError::bad_request(format!("reading upload: {e}")))?;
        if n == 0 {
            break;
        }
        let next = size + n as u64;
        if next > max {
            return Err(ApiError::bad_request("object is larger than 1 GiB"));
        }
        hasher.update(&buf[..n]);
        out.extend_from_slice(&buf[..n]);
        size = next;
    }
    Ok((out, hex(&hasher.finalize()), size))
}

#[async_trait]
impl ImageStore for SiloStore {
    fn name(&self) -> &'static str {
        "silo"
    }

    async fn put(&self, mut data: ImageData) -> ApiResult<Stored> {
        let (bytes, sha256, size) = read_blob(&mut data).await?;
        let key = format!("sha256/{sha256}");
        let bucket_id = self.ensure_bucket()?;
        let obj = self
            .client
            .put_object(
                &bucket_id,
                &key,
                &bytes,
                Some("application/octet-stream".to_owned()),
            )
            .await?;
        if obj.status.sha256 != sha256 || obj.status.size_bytes != size {
            let _ = self.client.delete_object(&bucket_id, &key).await;
            return Err(ApiError::internal(
                "silo object does not match the uploaded bytes",
            ));
        }
        Ok(Stored {
            reference: format!("{REF_PREFIX}{sha256}"),
            size,
            sha256,
        })
    }

    async fn open(&self, reference: &str) -> ApiResult<ImageData> {
        let key = object_key(reference)?;
        let bucket = self.client.get_bucket(None, &self.bucket).map_err(|err| {
            if missing(&err) {
                ApiError::not_found(format!("image data {reference} not found"))
            } else {
                err
            }
        })?;
        let (_obj, bytes) = self
            .client
            .get_object(&bucket.meta.id, &key)
            .await
            .map_err(|err| {
                if missing(&err) {
                    ApiError::not_found(format!("image data {reference} not found"))
                } else {
                    err
                }
            })?;
        Ok(Box::pin(std::io::Cursor::new(bytes)))
    }

    async fn delete(&self, reference: &str) -> ApiResult<()> {
        let key = object_key(reference)?;
        let bucket = match self.client.get_bucket(None, &self.bucket) {
            Ok(bucket) => bucket,
            Err(err) if missing(&err) => return Ok(()),
            Err(err) => return Err(err),
        };
        match self.client.delete_object(&bucket.meta.id, &key).await {
            Ok(_) => Ok(()),
            Err(err) if missing(&err) => Ok(()),
            Err(err) => Err(err),
        }
    }

    fn local_path(&self, reference: &str) -> Option<PathBuf> {
        let key = object_key(reference).ok()?;
        let bucket = self.client.get_bucket(None, &self.bucket).ok()?;
        self.client.key_path(&bucket.meta.id, &key).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{CreateImage, ImageFormat, ImageSpec, Visibility};
    use pistack_store::Store;
    use pistack_types::error::ErrorCode;
    use pistack_types::Phase;
    use std::io;
    use std::pin::Pin;
    use std::sync::Arc;
    use std::task::{Context, Poll};
    use tokio::io::{AsyncRead, ReadBuf};

    fn sample(client: &crate::Client, name: &str, format: ImageFormat) -> crate::Image {
        client
            .create(CreateImage {
                name: name.to_owned(),
                description: String::new(),
                labels: Default::default(),
                annotations: Default::default(),
                project: None,
                spec: ImageSpec {
                    format,
                    os: None,
                    arch: "x86_64".into(),
                    min_disk_gib: 1,
                    min_ram_mib: 0,
                    visibility: Visibility::Private,
                    source_url: None,
                },
            })
            .expect("create image")
    }

    fn files_under(dir: &std::path::Path) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.flatten()
            .map(|e| {
                let path = e.path();
                if path.is_dir() {
                    files_under(&path)
                } else {
                    1
                }
            })
            .sum()
    }

    struct FailRead;

    impl AsyncRead for FailRead {
        fn poll_read(
            self: Pin<&mut Self>,
            _cx: &mut Context<'_>,
            _buf: &mut ReadBuf<'_>,
        ) -> Poll<io::Result<()>> {
            Poll::Ready(Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "upload broke",
            )))
        }
    }

    #[test]
    fn key_is_sha256_slash_hex() {
        let hexd = "ab".repeat(32);
        assert_eq!(
            object_key(&format!("sha256:{hexd}")).unwrap(),
            format!("sha256/{hexd}")
        );
        assert_eq!(
            object_key(&format!("sha256/{hexd}")).unwrap(),
            format!("sha256/{hexd}")
        );
        let upper = "AB".repeat(32);
        assert_eq!(
            object_key(&format!("sha256:{upper}")).unwrap(),
            format!("sha256/{hexd}")
        );
        assert!(object_key("sha256:abcd").is_err());
        assert!(object_key("md5:nope").is_err());
        assert!(object_key(&format!("sha256:{}", "gg".repeat(32))).is_err());
    }

    #[tokio::test]
    async fn put_open_delete_roundtrip() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::in_memory().expect("store");
        let root = dir.path().join("silo");
        let objects = ::silo::Client::new(store, &root);
        let blobs = SiloStore::new(objects.clone(), "");
        assert_eq!(blobs.bucket, DEFAULT_BUCKET);

        let payload = b"pistack-image-bytes".to_vec();
        let stored = blobs
            .put(Box::pin(std::io::Cursor::new(payload.clone())))
            .await
            .expect("put");
        let sha = stored.sha256.clone();
        assert_eq!(sha.len(), 64);
        assert_eq!(stored.reference, format!("sha256:{sha}"));
        assert_eq!(stored.size, payload.len() as u64);

        let bucket = objects.get_bucket(None, DEFAULT_BUCKET).expect("bucket");
        let key = format!("sha256/{sha}");
        let (obj, got) = objects
            .get_object(&bucket.meta.id, &key)
            .await
            .expect("silo get");
        assert_eq!(got, payload);
        assert_eq!(obj.spec.key, key);
        assert_eq!(obj.status.sha256, sha);

        let disk = root.join(bucket.meta.id.to_string()).join(&key);
        assert_eq!(std::fs::read(&disk).expect("object file"), payload);
        let local = blobs.local_path(&stored.reference).expect("local path");
        assert_eq!(local, disk);

        let mut stream = blobs.open(&stored.reference).await.expect("open");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read");
        assert_eq!(buf, payload);

        // Same bytes share one key. A second put does not add a file.
        let again = blobs
            .put(Box::pin(std::io::Cursor::new(payload.clone())))
            .await
            .expect("put again");
        assert_eq!(again.reference, stored.reference);
        assert_eq!(files_under(&root), 1);

        blobs.delete(&stored.reference).await.expect("delete");
        blobs.delete(&key).await.expect("delete is idempotent");
        assert!(!disk.exists());
        match blobs.open(&stored.reference).await {
            Err(err) => assert_eq!(err.code, ErrorCode::NotFound),
            Ok(_) => panic!("open after delete"),
        }
        assert!(blobs.delete("not-a-ref").await.is_err());
    }

    #[tokio::test]
    async fn upload_and_failed_put_leave_pending_or_error() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::in_memory().expect("store");
        let root = dir.path().join("silo");
        let objects = ::silo::Client::new(store.clone(), &root);
        let client = crate::Client::new(
            store,
            Arc::new(SiloStore::new(objects.clone(), DEFAULT_BUCKET)),
        );

        let payload = b"pistack-raw-fixture".to_vec();
        let img = sample(&client, "raw-one", ImageFormat::Raw);
        assert_eq!(img.status.phase, Phase::Pending);
        let ready = client
            .upload(
                &img.meta.id,
                Box::pin(std::io::Cursor::new(payload.clone())),
            )
            .await
            .expect("upload");
        assert_eq!(ready.status.phase, Phase::Ready);
        let sha = ready.status.sha256.clone().expect("sha");
        assert_eq!(
            ready.status.backing_ref.as_deref(),
            Some(format!("sha256:{sha}").as_str())
        );
        let bucket = objects.get_bucket(None, DEFAULT_BUCKET).expect("bucket");
        let key = format!("sha256/{sha}");
        let (_obj, got) = objects
            .get_object(&bucket.meta.id, &key)
            .await
            .expect("object");
        assert_eq!(got, payload);
        assert!(ready
            .status
            .file_path
            .as_ref()
            .is_some_and(|p| { std::path::Path::new(p).is_file() }));

        let img2 = sample(&client, "raw-two", ImageFormat::Raw);
        let ready2 = client
            .upload(
                &img2.meta.id,
                Box::pin(std::io::Cursor::new(payload.clone())),
            )
            .await
            .expect("second upload");
        assert_eq!(ready2.status.backing_ref, ready.status.backing_ref);
        client.delete(&img.meta.id).await.expect("delete one");
        assert!(
            root.join(bucket.meta.id.to_string()).join(&key).is_file(),
            "shared blob stays while another image references it"
        );
        let mut stream = client.open(&img2.meta.id).await.expect("still open");
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.expect("read");
        assert_eq!(buf, payload);

        // A broken stream never calls put_object, so no new key appears.
        let broken = sample(&client, "broken", ImageFormat::Raw);
        let err = client
            .upload(&broken.meta.id, Box::pin(FailRead))
            .await
            .expect_err("broken upload");
        assert_eq!(err.code, ErrorCode::BadRequest);
        let broken = client.images().get(&broken.meta.id).expect("record");
        assert!(matches!(broken.status.phase, Phase::Pending | Phase::Error));
        assert_ne!(broken.status.phase, Phase::Creating);
        assert_ne!(broken.status.phase, Phase::Ready);

        // The data directory is not a directory, so put_object fails.
        let bad_root = dir.path().join("not-a-directory");
        std::fs::write(&bad_root, b"x").expect("file");
        let bad_store = Store::in_memory().expect("store");
        let bad = crate::Client::new(
            bad_store.clone(),
            Arc::new(SiloStore::new(
                ::silo::Client::new(bad_store, &bad_root),
                DEFAULT_BUCKET,
            )),
        );
        let img = sample(&bad, "cannot-write", ImageFormat::Raw);
        assert!(bad
            .upload(&img.meta.id, Box::pin(std::io::Cursor::new(payload)))
            .await
            .is_err());
        let img = bad.images().get(&img.meta.id).expect("record");
        assert!(matches!(img.status.phase, Phase::Pending | Phase::Error));
        assert_ne!(img.status.phase, Phase::Ready);
        assert!(bad_root.is_file(), "failed put does not replace the path");
    }

    fn qcow2_header(backing_offset: u64, backing_size: u32, size: u64) -> Vec<u8> {
        let mut h = vec![0u8; 65536];
        h[..4].copy_from_slice(b"QFI\xfb");
        h[4..8].copy_from_slice(&3u32.to_be_bytes());
        h[8..16].copy_from_slice(&backing_offset.to_be_bytes());
        h[16..20].copy_from_slice(&backing_size.to_be_bytes());
        h[20..24].copy_from_slice(&16u32.to_be_bytes());
        h[24..32].copy_from_slice(&size.to_be_bytes());
        h
    }

    #[tokio::test]
    async fn qcow2_inspect_stays_and_backing_file_is_not_kept() {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = Store::in_memory().expect("store");
        let root = dir.path().join("silo");
        let client = crate::Client::new(
            store.clone(),
            Arc::new(SiloStore::new(
                ::silo::Client::new(store, &root),
                DEFAULT_BUCKET,
            )),
        );

        let mut evil = qcow2_header(104, 11, 1 << 30);
        evil[104..115].copy_from_slice(b"/etc/shadow");
        let img = sample(&client, "evil", ImageFormat::Qcow2);
        let err = client
            .upload(&img.meta.id, Box::pin(std::io::Cursor::new(evil)))
            .await
            .expect_err("backing file");
        assert_eq!(err.code, ErrorCode::Unprocessable);
        let img_after = client.images().get(&img.meta.id).expect("record");
        assert_eq!(img_after.status.phase, Phase::Error);
        assert_eq!(files_under(&root), 0, "rejected image must not be kept");

        let clean = if crate::inspect::qemu_img_available() {
            let p = dir.path().join("clean.qcow2");
            let ok = std::process::Command::new("qemu-img")
                .args(["create", "-f", "qcow2"])
                .arg(&p)
                .arg("4G")
                .status()
                .expect("qemu-img create")
                .success();
            assert!(ok);
            std::fs::read(&p).expect("read qcow2")
        } else {
            qcow2_header(0, 0, 4 << 30)
        };
        let ready = client
            .upload(&img.meta.id, Box::pin(std::io::Cursor::new(clean)))
            .await
            .expect("clean qcow2");
        assert_eq!(ready.status.phase, Phase::Ready);
        assert_eq!(ready.status.virtual_size_bytes, Some(4 << 30));
        assert!(ready
            .status
            .backing_ref
            .as_ref()
            .is_some_and(|r| { r.starts_with("sha256:") && r.len() == "sha256:".len() + 64 }));
    }
}
