// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Block volumes, snapshots, backups (replaces Cinder).
//!
//! * [`types`] — `VolumeType`, `Volume`, `Snapshot`, `Backup`.
//! * [`backend`] — the `BlockBackend` seam and the `local`, `lvm`, `rbd`
//!   reference backends.
//! * [`Client`] — the module core; the only writer of `status`. `forge`
//!   uses [`Client::attach_info`], [`Client::mark_attached`] and
//!   [`Client::mark_detached`].
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
mod place;
pub mod quota;
pub mod reconcile;
pub mod types;
mod usage;

pub use backend::{AttachInfo, AttachTarget, BackendRef, BlockBackend, LocalFileBackend, Protocol};
pub use client::{Client, Create, UpdateMeta};
pub use quota::{AllowAll, QuotaCheck, QuotaRequest};
pub use types::*;

/// The slab module. `Default` runs the local file backend under
/// `$PISTACK_SLAB_DIR`, else `$PISTACK_DATA_DIR/volumes`, else
/// `./data/volumes`.
pub struct Slab {
    backend: Arc<dyn BlockBackend>,
    quota: Arc<dyn QuotaCheck>,
    queue: Option<CommandQueue>,
}

impl Default for Slab {
    fn default() -> Self {
        Self::new(
            Arc::new(LocalFileBackend::new(default_volume_dir())),
            Arc::new(AllowAll),
        )
    }
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
        }
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

    /// A `Client` over `store` using this module's backend and quota hook.
    /// Other modules get theirs from here.
    pub fn client(&self, store: Store) -> Client {
        Client::new(store, self.backend.clone(), self.quota.clone())
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
        client.seed_defaults()?;
        tokio::spawn(reconcile::run(
            client,
            ctx.events.subscribe(),
            ctx.shutdown.clone(),
        ));
        Ok(())
    }
}
