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
//! * [`usage`] — the project's `images` and `images_bytes` quotas.
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
pub mod usage;

pub use client::{Client, CreateImage, UpdateImage};
pub use store::{ImageData, ImageStore, LocalDirStore, SiloStore, Stored};
pub use types::*;

/// The mold module. `Default` stores blobs under `$PISTACK_MOLD_DIR`, else
/// `$PISTACK_DATA_DIR/images`, else `./data/images`.
pub struct Mold {
    blobs: Arc<dyn ImageStore>,
}

/// `$PISTACK_MOLD_DIR`, else `$PISTACK_DATA_DIR/images`, else `./data/images`.
pub fn default_image_dir() -> PathBuf {
    std::env::var_os("PISTACK_MOLD_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("PISTACK_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("./data"))
                .join("images")
        })
}

/// Module settings under `[modules.mold]` / `PISTACK__MODULES__MOLD__*`.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct MoldConfig {
    /// Where image bytes are kept: `local` (the image directory on this
    /// server, the default) or `silo` (a silo bucket, which the other
    /// control-plane servers copy).
    pub store: String,
    /// Bucket for `store = "silo"`. Empty selects `pistack-images`.
    pub bucket: String,
}

impl Default for MoldConfig {
    fn default() -> Self {
        Self {
            store: "local".to_owned(),
            bucket: String::new(),
        }
    }
}

impl MoldConfig {
    /// Reject a `store` that is neither `local` nor `silo`.
    pub fn validate(&self) -> Result<(), pistack_config::ConfigError> {
        match self.store.trim() {
            "local" | "silo" => Ok(()),
            other => Err(pistack_config::ConfigError::Module {
                module: "mold".into(),
                message: format!("store must be 'local' or 'silo', not '{other}'"),
            }),
        }
    }

    /// Read `[modules.mold]` from a loaded config. Missing section → defaults.
    pub fn from_config(cfg: &pistack_config::Config) -> Result<Self, pistack_config::ConfigError> {
        let mold: Self = cfg.module("mold")?;
        mold.validate()?;
        Ok(mold)
    }

    /// Load process config (file + `PISTACK__*`) and take `[modules.mold]`.
    pub fn load() -> Result<Self, pistack_config::ConfigError> {
        let cfg = pistack_config::Config::load(None)?;
        Self::from_config(&cfg)
    }

    /// The blob store these settings name. `silo` is the client used for
    /// `store = "silo"`; images already in the other store are not moved.
    pub fn image_store(&self, silo: ::silo::Client) -> Arc<dyn ImageStore> {
        match self.store.trim() {
            "silo" => Arc::new(SiloStore::new(silo, self.bucket.trim())),
            _ => Arc::new(LocalDirStore::new(default_image_dir())),
        }
    }
}

impl Default for Mold {
    fn default() -> Self {
        Self::new(Arc::new(LocalDirStore::new(default_image_dir())))
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
            version: pistack_types::BUILD,
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

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn the_store_is_local_unless_silo_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let silo = ::silo::Client::new(Store::in_memory().unwrap(), dir.path());

        let cfg = pistack_config::Config::from_toml("").unwrap();
        let mold = MoldConfig::from_config(&cfg).unwrap();
        assert_eq!(mold, MoldConfig::default());
        assert_eq!(mold.image_store(silo.clone()).name(), "local");

        let cfg = pistack_config::Config::from_toml(
            "[modules.mold]\nstore = \"silo\"\nbucket = \"golden\"\n",
        )
        .unwrap();
        let mold = MoldConfig::from_config(&cfg).unwrap();
        assert_eq!(mold.bucket, "golden");
        assert_eq!(mold.image_store(silo).name(), "silo");

        let cfg = pistack_config::Config::from_toml("[modules.mold]\nstore = \"ceph\"\n").unwrap();
        let err = MoldConfig::from_config(&cfg).unwrap_err();
        assert!(err.to_string().contains("'local' or 'silo'"), "{err}");
    }
}
