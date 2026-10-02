// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Content-addressed blobs on local disk.
//!
//! Layout: `<dir>/blobs/sha256/<aa>/<sha256hex>`, where `aa` is the first
//! two hex digits. Uploads stream into `<dir>/tmp/<uuid>` and are renamed
//! into place once the hash is known, so a partial upload is never
//! visible and two identical uploads share one file.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use super::{hex, ImageData, ImageStore, Stored};

pub const REF_PREFIX: &str = "sha256:";

pub struct LocalDirStore {
    dir: PathBuf,
}

impl LocalDirStore {
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self { dir: dir.into() }
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Path for a `sha256:<hex>` reference, or `None` if malformed.
    pub fn blob_path(&self, reference: &str) -> Option<PathBuf> {
        let hexd = reference.strip_prefix(REF_PREFIX)?;
        if hexd.len() != 64 || !hexd.chars().all(|c| c.is_ascii_hexdigit()) {
            return None;
        }
        let hexd = hexd.to_ascii_lowercase();
        Some(
            self.dir
                .join("blobs")
                .join("sha256")
                .join(&hexd[..2])
                .join(hexd),
        )
    }

    fn path_for(&self, reference: &str) -> ApiResult<PathBuf> {
        self.blob_path(reference).ok_or_else(|| {
            ApiError::bad_request(format!("malformed image reference '{reference}'"))
        })
    }
}

fn io_err(e: io::Error) -> ApiError {
    ApiError::internal(format!("image store: {e}"))
}

#[async_trait]
impl ImageStore for LocalDirStore {
    fn name(&self) -> &'static str {
        "local"
    }

    async fn put(&self, mut data: ImageData) -> ApiResult<Stored> {
        let tmp_dir = self.dir.join("tmp");
        fs::create_dir_all(&tmp_dir).await.map_err(io_err)?;
        let tmp = tmp_dir.join(uuid::Uuid::now_v7().to_string());
        let mut file = fs::File::create(&tmp).await.map_err(io_err)?;

        let result: ApiResult<Stored> = async {
            let mut hasher = Sha256::new();
            let mut size: u64 = 0;
            let mut buf = vec![0u8; 256 * 1024];
            loop {
                let n = data
                    .read(&mut buf)
                    .await
                    .map_err(|e| ApiError::bad_request(format!("reading upload: {e}")))?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
                file.write_all(&buf[..n]).await.map_err(io_err)?;
                size += n as u64;
            }
            file.flush().await.map_err(io_err)?;
            file.sync_all().await.map_err(io_err)?;
            let sha256 = hex(&hasher.finalize());
            let reference = format!("{REF_PREFIX}{sha256}");
            let dest = self.path_for(&reference)?;
            if let Some(parent) = dest.parent() {
                fs::create_dir_all(parent).await.map_err(io_err)?;
            }
            if fs::try_exists(&dest).await.map_err(io_err)? {
                // Same content already stored; keep the existing blob.
                fs::remove_file(&tmp).await.map_err(io_err)?;
            } else {
                fs::rename(&tmp, &dest).await.map_err(io_err)?;
            }
            Ok(Stored {
                reference,
                size,
                sha256,
            })
        }
        .await;

        if result.is_err() {
            let _ = fs::remove_file(&tmp).await;
        }
        result
    }

    async fn open(&self, reference: &str) -> ApiResult<ImageData> {
        let path = self.path_for(reference)?;
        match fs::File::open(&path).await {
            Ok(f) => Ok(Box::pin(f)),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(ApiError::not_found(format!(
                "image data {reference} not found"
            ))),
            Err(e) => Err(io_err(e)),
        }
    }

    async fn delete(&self, reference: &str) -> ApiResult<()> {
        let path = self.path_for(reference)?;
        match fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }

    fn local_path(&self, reference: &str) -> Option<PathBuf> {
        self.blob_path(reference)
    }
}
