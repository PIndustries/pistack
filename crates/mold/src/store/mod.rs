// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Where image bytes live. `mold` keeps metadata in the state store and
//! hands the bytes to an `ImageStore`: local disk today, `silo` (S3) later.

use async_trait::async_trait;
use pistack_types::ApiResult;
use std::path::PathBuf;
use std::pin::Pin;
use tokio::io::AsyncRead;

pub mod local;
pub mod silo;

pub use local::LocalDirStore;
pub use silo::SiloStore;

/// A readable byte stream. Boxed so handlers, backends and other modules
/// can pass it around without naming the concrete reader.
pub type ImageData = Pin<Box<dyn AsyncRead + Send + 'static>>;

/// Result of storing a blob.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Stored {
    /// Store reference, `sha256:<hex>` for content-addressed stores.
    pub reference: String,
    pub size: u64,
    /// Lower-case hex digest of the bytes written.
    pub sha256: String,
}

#[async_trait]
pub trait ImageStore: Send + Sync + 'static {
    fn name(&self) -> &'static str;

    /// Write the stream to durable storage, hashing as it goes. The blob is
    /// visible under the returned reference only after the whole stream
    /// has been consumed.
    async fn put(&self, data: ImageData) -> ApiResult<Stored>;

    /// Stream a stored blob.
    async fn open(&self, reference: &str) -> ApiResult<ImageData>;

    /// Remove a blob. Idempotent.
    async fn delete(&self, reference: &str) -> ApiResult<()>;

    /// Local filesystem path of the blob, when the store has one. Lets the
    /// inspector run `qemu-img` directly instead of streaming a copy.
    fn local_path(&self, _reference: &str) -> Option<PathBuf> {
        None
    }
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        // Writing to a String cannot fail.
        let _ = write!(s, "{b:02x}");
    }
    s
}
