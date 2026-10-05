// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object storage.
//!
//! Not a full S3 server. Buckets are store resources; object bytes live on
//! local disk under `$PISTACK_DATA_DIR/silo/{bucket-id}/{key}`, and
//! `status.sha256` is the content address of those bytes. Object puts stream
//! to a temp file then rename; the default size cap is 5 GiB
//! ([`MAX_OBJECT_BYTES`]), overridable via [`SiloConfig`] /
//! `[modules.silo] max_object_bytes`.
//!
//! Path-style S3 routes (`/{bucket}/{key}`, ListObjectsV2, multipart
//! upload) share this module's HTTP router and the same bearer/`Caller`
//! auth. **AWS Signature Version 4 is not verified**; clients must present
//! a gate token (or run under the gateway's `--dev` admin injection).
//!
//! Multipart staging lives under `{data}/silo/{bucket}/.uploads/{uploadId}/`.
//! Abort and complete delete that directory; **stale uploads are not GC'd**
//! yet if a client disappears mid-upload.
//!
//! * [`types`] — `Bucket`, `Object`, [`SiloConfig`].
//! * [`Client`] — the library API.
//! * [`http`] — native routes mounted at `/silo`.
//! * [`s3`] — path-style S3-compatible handlers on the same router.
//! * [`multipart`] — Create/Upload/Complete/Abort multipart helpers.

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::path::PathBuf;

pub mod client;
pub mod http;
pub mod multipart;
pub mod s3;
pub mod types;

pub use client::Client;
pub use types::*;

/// Object storage module.
///
/// The binary builds this with [`Silo::configured`] after loading
/// [`SiloConfig`] at assemble time. [`Default`] keeps the 5 GiB cap for
/// tests and ad-hoc mounts.
#[derive(Clone, Default)]
pub struct Silo {
    config: SiloConfig,
}

impl Silo {
    /// `$PISTACK_DATA_DIR/silo`, or `./data/silo` when the variable is unset.
    pub fn data_root() -> PathBuf {
        std::env::var_os("PISTACK_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data"))
            .join("silo")
    }

    /// Module with an explicit size limit (and any future silo settings).
    pub fn configured(config: SiloConfig) -> Self {
        Self { config }
    }

    /// Settings the module was built with.
    pub fn config(&self) -> &SiloConfig {
        &self.config
    }

    /// Client using this module's config and [`Self::data_root`].
    pub fn client(&self, store: Store) -> Client {
        Client::new(store, Self::data_root()).with_max_object_bytes(self.config.max_object_bytes)
    }
}

#[async_trait]
impl Module for Silo {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "silo",
            prefix: "silo",
            summary: "Object storage, S3 API",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        let store = ctx.store::<Store>();
        http::router(self.client((*store).clone()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_store::Store;
    use tempfile::tempdir;
    use tokio::sync::broadcast;

    #[test]
    fn configured_max_reaches_client() {
        let dir = tempdir().unwrap();
        let (events, _) = broadcast::channel(16);
        let store = Store::open(dir.path().join("state.redb"), events).unwrap();
        let max = 64 * 1024;
        let silo = Silo::configured(SiloConfig {
            max_object_bytes: max,
        });
        assert_eq!(silo.client(store).max_object_bytes(), max);
    }
}
