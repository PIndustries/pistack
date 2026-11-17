// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Shared filesystems (NFS and CephFS).
//!
//! A [`Share`] is a name, a protocol (`nfs` or `cephfs`), a size and an
//! access list. NFS with a client CIDR is `Ready` only after the managed
//! exports file is reloaded with `exportfs -ra`. CephFS is `Ready` only
//! after `ceph fs subvolume create` succeeds. A failed NFS export is
//! `Error` and the reconcile loop retries it. A missing Ceph tool stays
//! `Pending`.
//!
//! HTTP is mounted at `/shelf`: `GET|POST /shares`, `GET|PUT|DELETE /shares/{id}`.

mod export_address;
mod export_opts;
mod http;
mod realize;
mod share_dir;
pub(crate) mod spawn_retry;
mod types;

pub use realize::quiesce;
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use pistack_store::Store;
use pistack_types::{kind, Module, ModuleContext, ModuleInfo};

/// The shelf module. Routes read the store from [`ModuleContext`].
#[derive(Default)]
pub struct Shelf;

impl Shelf {
    fn store(ctx: &ModuleContext) -> Store {
        let store = ctx.store::<Store>();
        (*store).clone()
    }

    /// A ready share reports `<address>:<path>`; the address is the host of
    /// this server's public URL. `bin/pistack` keeps
    /// PISTACK_EXPORT_ADDRESS at the same host for anything that runs shelf
    /// outside this process. A URL with no host is an empty set: this server
    /// has no address, and does not borrow one from the environment.
    pub fn set_export_address(&self, public_url: &str) {
        let host = export_opts::address_of(public_url);
        export_address::set(host.as_deref().unwrap_or(""));
    }
}

#[async_trait]
impl Module for Shelf {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "shelf",
            prefix: "shelf",
            summary: "Shared filesystems",
            version: pistack_types::BUILD,
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        http::router(Self::store(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        self.set_export_address(&ctx.public_url);
        let store = Self::store(&ctx);
        match realize::ensure_share_root().await {
            Ok(root) => tracing::info!(root = %root.display(), "shelf share root ready"),
            Err(e) => tracing::warn!(error = %e, "shelf share root is not ready"),
        }
        let shutdown = ctx.shutdown.clone();
        let interval = realize::retry_interval();
        let leadership = ctx.leadership.clone();
        let events = ctx.events.clone();
        tokio::spawn(async move {
            leadership
                .run_while_leader(shutdown, move |active| {
                    let store = store.clone();
                    let events = events.clone();
                    async move {
                        let mut rx = events.subscribe();
                        // The first tick is immediate. The pass below is the
                        // restart reconcile; the next tick is one full
                        // interval later.
                        let mut tick = tokio::time::interval(interval);
                        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        tick.tick().await;
                        if let Err(e) = realize::reconcile_due(&store).await {
                            pistack_types::tally::reconcile_failed("shelf");
                            tracing::warn!(error = %e, "shelf: initial reconcile failed");
                        }
                        loop {
                            tokio::select! {
                                biased;
                                _ = active.cancelled() => {
                                    tracing::debug!("shelf reconciler stopping");
                                    break;
                                }
                                ev = rx.recv() => match ev {
                                    Ok(ev) if ev.kind == kind::SHARE => {
                                        if let Err(e) = realize::reconcile_due(&store).await {
                                            pistack_types::tally::reconcile_failed("shelf");
                                            tracing::warn!(error = %e, "shelf: reconcile failed");
                                        }
                                    }
                                    Ok(_) => {}
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!(missed = n, "shelf reconciler lagged");
                                        if let Err(e) = realize::reconcile_due(&store).await {
                                            pistack_types::tally::reconcile_failed("shelf");
                                            tracing::warn!(error = %e, "shelf: reconcile failed");
                                        }
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                },
                                _ = tick.tick() => {
                                    if active.is_cancelled() {
                                        break;
                                    }
                                    if let Err(e) = realize::reconcile_due(&store).await {
                                        tracing::warn!(error = %e, "shelf: retry failed");
                                    }
                                }
                            }
                        }
                    }
                })
                .await;
        });
        Ok(())
    }
}
