// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Block volumes, snapshots, backups (replaces Cinder).
//!
//! * [`types`] — `VolumeType`, `Volume`, `Snapshot`, `Backup`.
//! * [`backend`] — the `BlockBackend` seam and the `local`, `lvm`, `rbd`
//!   reference backends.
//! * [`Client`] — the module core; the only writer of `status`. `forge`
//!   uses [`Client::attach_info`], [`Client::begin_attach`],
//!   [`Client::confirm_attach`] and [`Client::mark_detached`].
//! * [`quota`] — optional extra `QuotaCheck`. Project `volumes` quotas are
//!   enforced in [`Client`] through `gate`.
//! * HTTP under `/volume`: `/volumes`, `/snapshots`, `/backups`,
//!   `/volume-types`.

use async_trait::async_trait;
use axum::Router;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub mod backend;
pub mod client;
pub mod http;
mod image;
mod place;
pub mod quota;
pub mod reconcile;
pub mod types;
mod usage;

pub use backend::{AttachInfo, AttachTarget, BackendRef, BlockBackend, LocalFileBackend, Protocol};
pub use client::{Client, Create, RestoreRequest, UpdateMeta};
pub use quota::{AllowAll, QuotaCheck, QuotaRequest};
pub use types::*;

/// The slab module. `Default` runs the local file backend under
/// `$PISTACK_SLAB_DIR`, else `$PISTACK_DATA_DIR/volumes`, else
/// `./data/volumes`.
pub struct Slab {
    backend: Arc<dyn BlockBackend>,
    quota: Arc<dyn QuotaCheck>,
    queue: Option<CommandQueue>,
    /// Shared with mold when the server wires one store. `None` uses
    /// [`mold::default_image_dir`].
    images: Option<Arc<dyn mold::ImageStore>>,
    /// Same cap silo uses for a single PUT. Backup parts stay within it.
    max_object_bytes: usize,
    /// Minimum size of every backup part except the last.
    min_part_bytes: usize,
}

impl Default for Slab {
    fn default() -> Self {
        Self::new(
            Arc::new(LocalFileBackend::new(default_volume_dir())),
            Arc::new(AllowAll),
        )
    }
}

/// `$PISTACK_DATA_DIR/slab-spool`, else `./data/slab-spool`.
pub(crate) fn default_spool_dir() -> PathBuf {
    std::env::var_os("PISTACK_DATA_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./data"))
        .join("slab-spool")
}

/// `$PISTACK_SLAB_DIR`, else `$PISTACK_DATA_DIR/volumes`, else `./data/volumes`.
pub(crate) fn default_volume_dir() -> PathBuf {
    std::env::var_os("PISTACK_SLAB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            std::env::var_os("PISTACK_DATA_DIR")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("./data"))
                .join("volumes")
        })
}

pub(crate) fn path_inside(root: &Path, path: &Path) -> bool {
    path.starts_with(root)
}

impl Slab {
    pub fn new(backend: Arc<dyn BlockBackend>, quota: Arc<dyn QuotaCheck>) -> Self {
        Self {
            backend,
            quota,
            queue: None,
            images: None,
            max_object_bytes: silo::MAX_OBJECT_BYTES,
            min_part_bytes: silo::MIN_PART_BYTES,
        }
    }

    /// Use the same object cap and minimum part size as the silo module.
    pub fn with_silo_limits(mut self, max_object_bytes: usize, min_part_bytes: usize) -> Self {
        self.max_object_bytes = max_object_bytes;
        self.min_part_bytes = min_part_bytes;
        self
    }

    pub fn with_quota(mut self, quota: Arc<dyn QuotaCheck>) -> Self {
        self.quota = quota;
        self
    }

    /// Hotplug attach and detach through this agent queue.
    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Blob store to read when a volume is created from an image. The
    /// server passes the same store mold uses.
    pub fn with_images(mut self, images: Arc<dyn mold::ImageStore>) -> Self {
        self.images = Some(images);
        self
    }

    /// A `Client` over `store` using this module's backend and quota hook.
    /// Other modules get theirs from here.
    pub fn client(&self, store: Store) -> Client {
        let mut client = Client::new(store, self.backend.clone(), self.quota.clone())
            .with_spool_dir(default_spool_dir())
            .with_silo_limits(self.max_object_bytes, self.min_part_bytes);
        if let Some(images) = &self.images {
            client = client.with_images(images.clone());
        }
        client
    }

    fn client_from(&self, ctx: &ModuleContext) -> Client {
        let store: Arc<Store> = ctx.store::<Store>();
        let client = self.client((*store).clone());
        match &self.queue {
            Some(queue) => client.with_queue(queue.clone()),
            None => client,
        }
    }
}

#[async_trait]
impl Module for Slab {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "slab",
            prefix: "volume",
            summary: "Block volumes, snapshots, backups",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        http::router(self.client_from(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client_from(&ctx);
        tokio::spawn(reconcile::run(
            client,
            ctx.events.clone(),
            ctx.shutdown.clone(),
            ctx.leadership.clone(),
        ));
        Ok(())
    }
}
