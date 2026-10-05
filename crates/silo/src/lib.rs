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
//! Path-style S3 routes (`/{bucket}/{key}`, ListObjectsV2) share this
//! module's HTTP router and the same bearer/`Caller` auth. **AWS Signature
//! Version 4 is not verified**; clients must present a gate token (or run
//! under the gateway's `--dev` admin injection).
//!
//! * [`types`] — `Bucket`, `Object`, [`SiloConfig`].
//! * [`Client`] — the library API.
//! * [`http`] — native routes mounted at `/silo`.
//! * [`s3`] — path-style S3-compatible handlers on the same router.

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::path::PathBuf;

pub mod client;
pub mod http;
pub mod s3;
pub mod types;

pub use client::Client;
pub use types::*;

/// Object storage module. `silo::Silo` is a unit value so the gateway can
/// keep constructing it that way. The data directory is read when the
/// router is built; size limits come from [`SiloConfig::load`].
#[derive(Default)]
pub struct Silo;

impl Silo {
    /// `$PISTACK_DATA_DIR/silo`, or `./data/silo` when the variable is unset.
    pub fn data_root() -> PathBuf {
        std::env::var_os("PISTACK_DATA_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("./data"))
            .join("silo")
    }

    pub fn client(&self, store: Store) -> Client {
        let cfg = SiloConfig::load();
        Client::new(store, Self::data_root()).with_max_object_bytes(cfg.max_object_bytes)
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
        let cfg = SiloConfig::load();
        http::router(
            Client::new((*store).clone(), Self::data_root())
                .with_max_object_bytes(cfg.max_object_bytes),
        )
    }
}
