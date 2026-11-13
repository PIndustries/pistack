// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Object storage.
//!
//! Not a full S3 server. Buckets are store resources; object bytes live on
//! local disk under `$PISTACK_DATA_DIR/silo/{bucket-id}/{key}`, and
//! `status.sha256` is the content address of those bytes. A put lands on
//! the leader's disk; the other control-plane servers pull the bytes in
//! the background and on a read that misses ([`replica`]). Object puts stream
//! to a temp file then rename; the default size cap is 5 GiB
//! ([`MAX_OBJECT_BYTES`]), overridable via [`SiloConfig`] /
//! `[modules.silo] max_object_bytes`.
//!
//! Path-style S3 routes (`/{bucket}/{key}`, ListObjectsV2, multipart
//! upload, ListMultipartUploads) share this module's HTTP router. Auth is
//! gate bearer `Caller` or **AWS Signature Version 4** via keep access keys
//! ([`sigv4`]): header auth, presigned GET/PUT, and aws-chunked streaming
//! PutObject/UploadPart (`STREAMING-AWS4-HMAC-SHA256-PAYLOAD`,
//! `STREAMING-UNSIGNED-PAYLOAD-TRAILER`,
//! `STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER`). **UploadPartCopy** and
//! **CopyObject** (`x-amz-copy-source`) are supported. Region comes from
//! `[modules.silo] region` (default `pistack`).
//!
//! Multipart staging lives under `{data}/silo/{bucket}/.uploads/{uploadId}/`.
//! Abort and complete delete that directory. Uploads a client never
//! finishes are swept once they are older than `[modules.silo]
//! multipart_ttl_secs` (default 7 days, see [`gc`]); the sweep runs at
//! startup and then periodically while this node is the cluster leader.
//! Staging is never a store record, so unfinished uploads are not objects.
//!
//! * [`types`] — `Bucket`, `Object`, [`SiloConfig`].
//! * [`Client`] — the library API.
//! * [`http`] — native routes mounted at `/silo`.
//! * [`s3`] — path-style S3-compatible handlers on the same router.
//! * [`multipart`] — Create/Upload/Complete/Abort/List multipart helpers.
//! * [`gc`] — the abandoned-upload sweep.
//! * [`replica`] — object bytes on more than one control-plane server.
//! * [`sigv4`] — AWS Signature Version 4 verification (in-process via keep).
//! * [`copy`] — UploadPartCopy source/range/conditional helpers.
//! * [`delimiter`] — shared ListObjectsV2 / ListMultipartUploads delimiter helpers.
//! * [`chunked`] — aws-chunked body decode (signed chunks and trailers).
//! * [`usage`] — the project's `buckets` quota.

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::path::PathBuf;

pub mod chunked;
pub mod client;
pub mod copy;
pub mod delimiter;
pub mod gc;
pub mod http;
pub mod multipart;
pub mod replica;
pub mod s3;
pub mod sigv4;
pub mod staging;
pub mod types;
pub mod usage;

pub use client::Client;
pub use replica::Peers;
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

    /// Client using this module's config and [`Self::data_root`]. It
    /// fetches bytes this server lacks from the other cluster members.
    pub fn client(&self, store: Store) -> Client {
        Client::new(store.clone(), Self::data_root())
            .with_max_object_bytes(self.config.max_object_bytes)
            .with_peers(Peers::from_cluster(store))
            .with_put_copies(self.config.put_copies)
    }
}

#[async_trait]
impl Module for Silo {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "silo",
            prefix: "silo",
            summary: "Object storage, S3 API",
            version: pistack_types::BUILD,
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        let store = ctx.store::<Store>();
        let client = self.client((*store).clone());
        let keep = match keep::Client::open(&store) {
            Ok(k) => Some(k),
            Err(e) => {
                tracing::warn!(error = %e, "silo: keep unavailable; SigV4 disabled");
                None
            }
        };
        http::router_with(client, keep, self.config.region.clone())
    }

    /// Abandoned multipart uploads are swept while this node is leader:
    /// once on promotion (startup on a sole server) and then periodically.
    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let store = ctx.store::<Store>();
        let client = self.client((*store).clone());
        let ttl = self.config.multipart_ttl();
        let leadership = ctx.leadership.clone();
        let shutdown = ctx.shutdown.clone();
        // Every server pulls, leader or not: after a failover the new
        // leader may still lack bytes the old one holds.
        if self.config.replicate_interval_secs > 0 {
            let leadership = ctx.leadership.clone();
            tokio::spawn(replica::run(
                client.clone(),
                std::time::Duration::from_secs(self.config.replicate_interval_secs),
                move || leadership.is_leader(),
                ctx.shutdown.clone(),
            ));
        }
        tokio::spawn(async move {
            leadership
                .run_while_leader(shutdown, move |active| gc::run(client.clone(), ttl, active))
                .await;
        });
        Ok(())
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
            ..SiloConfig::default()
        });
        assert_eq!(silo.client(store).max_object_bytes(), max);
    }
}
