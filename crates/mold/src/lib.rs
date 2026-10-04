// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Images: VM disks (raw, qcow2) and OCI bundles (replaces Glance).
//!
//! * [`types`] — `Image`, `ImageMember`.
//! * [`store`] — the `ImageStore` seam: `LocalDirStore` (content-addressed
//!   by sha256) and `SiloStore` (silo object API, key `sha256/<hex>`).
//! * [`inspect`] — `qemu-img info` wrapper with a header-parsing fallback;
//!   rejects images with backing files.
//! * [`Client`] — the module core; `forge` and `slab` use
//!   [`Client::open`] and [`Client::resolve`].
//! * HTTP under `/image`: `/images`, `/images/{id}/data`, `/images/{id}/members`.

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::path::PathBuf;
use std::sync::Arc;

pub mod client;
pub mod http;
pub mod inspect;
pub mod store;
pub mod types;

pub use client::{Client, CreateImage, UpdateImage};
pub use store::{ImageData, ImageStore, LocalDirStore, SiloStore, Stored};
pub use types::*;

/// The mold module. `Default` stores blobs under `$PISTACK_MOLD_DIR`, else
/// `$PISTACK_DATA_DIR/images`, else `./data/images`.
pub struct Mold {
    blobs: Arc<dyn ImageStore>,
}

impl Default for Mold {
    fn default() -> Self {
        let dir = std::env::var_os("PISTACK_MOLD_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                std::env::var_os("PISTACK_DATA_DIR")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("./data"))
                    .join("images")
            });
        Self::new(Arc::new(LocalDirStore::new(dir)))
    }
}

impl Mold {
    pub fn new(blobs: Arc<dyn ImageStore>) -> Self {
        Self { blobs }
    }

    /// A `Client` over `store` using this module's blob store. Other
    /// modules get theirs from here.
    pub fn client(&self, store: Store) -> Client {
        Client::new(store, self.blobs.clone())
    }

    fn client_from(&self, ctx: &ModuleContext) -> Client {
        let store: Arc<Store> = ctx.store::<Store>();
        self.client((*store).clone())
    }
}

#[async_trait]
impl Module for Mold {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "mold",
            prefix: "image",
            summary: "Boot images and templates",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        http::router(self.client_from(&ctx))
    }

    async fn start(&self, _ctx: ModuleContext) -> anyhow::Result<()> {
        if !inspect::qemu_img_available() {
            tracing::info!("qemu-img not found; image inspection falls back to header parsing");
        }
        Ok(())
    }
}
