// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Image blobs in `silo` (S3). Stub: every method returns `not_implemented`.
//!
//! Plan: one bucket (`pistack-images` by default), key `sha256/<hex>`.
//! `put` streams a multipart upload while hashing, then copies the
//! finished object to its content-addressed key and deletes the temporary
//! one (S3 has no rename). `open` is a streaming `GetObject`; `delete` a
//! `DeleteObject`. Credentials come from `keep`.

use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult};

use super::{ImageData, ImageStore, Stored};

pub struct SiloStore {
    pub bucket: String,
}

impl Default for SiloStore {
    fn default() -> Self {
        Self {
            bucket: "pistack-images".into(),
        }
    }
}

impl SiloStore {
    pub fn new(bucket: impl Into<String>) -> Self {
        Self {
            bucket: bucket.into(),
        }
    }

    fn todo(op: &str) -> ApiError {
        ApiError::not_implemented(format!("silo image store: {op} not implemented"))
    }
}

#[async_trait]
impl ImageStore for SiloStore {
    fn name(&self) -> &'static str {
        "silo"
    }

    async fn put(&self, _data: ImageData) -> ApiResult<Stored> {
        Err(Self::todo("put").with_detail("bucket", self.bucket.clone()))
    }

    async fn open(&self, reference: &str) -> ApiResult<ImageData> {
        Err(Self::todo("open").with_detail("key", reference))
    }

    async fn delete(&self, reference: &str) -> ApiResult<()> {
        Err(Self::todo("delete").with_detail("key", reference))
    }
}
