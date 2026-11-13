// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `beacon`: DNS zones and record sets, served by an embedded authoritative
//! server (hickory).
//!
//! Layout:
//! * [`types`] — `Zone`, `RecordSet`, validation and zone-file export.
//! * [`Client`] — validation and writes; the library API other modules use
//!   ([`Client::upsert_record`], [`Client::remove_record`]).
//! * [`server`] — the hickory request handler and UDP/TCP listener.
//! * [`xfer`] — secondary AXFR pull from configured primaries.
//! * [`auto`] — `<machine>.<zone>` records for zones labelled
//!   `pistack.io/auto=true`.
//! * `api` — HTTP, mounted at `/dns`.
//!
//! The serial reconciler and auto-record writer run only while this node is
//! the cluster leader. The authoritative DNS listener stays up on every node
//! so local lookups keep working from a replicated store.

pub mod api;
pub mod auto;
pub mod client;
pub mod server;
pub mod types;
pub mod xfer;

pub use client::Client;
pub use types::*;

use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::net::SocketAddr;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

/// Timer for a full serial-reconcile pass, in case an event was missed.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// The module. `Default` resolves listen addresses via [`server::listen_addrs`]
/// (see that function for the env / private-hint / loopback rules). Tests use
/// [`Beacon::with_listen`] and port 0. The binary passes the server `--listen`
/// through [`Beacon::with_cluster_hint`] so an unset env binds the cluster
/// private address when one is known.
#[derive(Default)]
pub struct Beacon {
    listen: Option<SocketAddr>,
    /// Server `--listen` (or similar) used when `PISTACK_DNS_LISTEN` is unset.
    cluster_hint: Option<String>,
    bound: Arc<OnceLock<SocketAddr>>,
}

impl Beacon {
    pub fn with_listen(addr: SocketAddr) -> Self {
        Self {
            listen: Some(addr),
            cluster_hint: None,
            bound: Arc::new(OnceLock::new()),
        }
    }

    /// Prefer the private address in `server_listen` when the env is unset.
    pub fn with_cluster_hint(server_listen: impl Into<String>) -> Self {
        Self {
            listen: None,
            cluster_hint: Some(server_listen.into()),
            bound: Arc::new(OnceLock::new()),
        }
    }

    /// A `Client` over the context's store.
    pub fn client(&self, ctx: &ModuleContext) -> Client {
        let store = ctx.store::<Store>();
        Client::new((*store).clone())
    }

    /// First address the DNS server bound to, once `start` has run.
    pub fn bound_addr(&self) -> Option<SocketAddr> {
        self.bound.get().copied()
    }
}

#[async_trait]
impl Module for Beacon {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "beacon",
            prefix: "dns",
            summary: "DNS zones and records, served authoritatively (hickory)",
            version: pistack_types::BUILD,
        }
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        api::router(self.client(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client(&ctx);
        let secondaries = server::SecondaryCache::new();

        let listens = match self.listen {
            Some(a) => vec![a],
            None => {
                let hint = self
                    .cluster_hint
                    .as_deref()
                    .or(Some(ctx.public_url.as_str()));
                server::listen_addrs(hint)?
            }
        };
        let bounds = server::serve(
            client.clone(),
            &listens,
            ctx.events.clone(),
            ctx.shutdown.clone(),
            secondaries.clone(),
        )
        .await?;
        if let Some(first) = bounds.first().copied() {
            let _ = self.bound.set(first);
        }

        // Secondary AXFR pull runs on every node that serves DNS so the local
        // secondary cache stays warm even when this node is not the leader.
        {
            let client = client.clone();
            let secondaries = secondaries.clone();
            let events = ctx.events.clone();
            let shutdown = ctx.shutdown.clone();
            tokio::spawn(async move {
                transfer_secondaries(&client, &secondaries).await;
                let mut rx = events.subscribe();
                let mut tick = tokio::time::interval(RETRY_INTERVAL);
                tick.tick().await;
                loop {
                    tokio::select! {
                        _ = shutdown.cancelled() => break,
                        ev = rx.recv() => match ev {
                            Ok(ev) if ev.kind == pistack_types::kind::ZONE => {
                                transfer_secondaries(&client, &secondaries).await;
                            }
                            Ok(_) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                                transfer_secondaries(&client, &secondaries).await;
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        },
                        _ = tick.tick() => {
                            transfer_secondaries(&client, &secondaries).await;
                        }
                    }
                }
            });
        }

        let leadership = ctx.leadership.clone();
        let shutdown = ctx.shutdown.clone();
        let events = ctx.events.clone();
        tokio::spawn(async move {
            leadership
                .run_while_leader(shutdown, move |active| {
                    let client = client.clone();
                    let events = events.clone();
                    async move {
                        if let Err(e) = client.reconcile_all() {
                            tracing::warn!(error = %e, "beacon: initial reconcile");
                        }
                        auto::start(
                            client.clone(),
                            events.subscribe(),
                            active.clone(),
                        );

                        let mut rx = events.subscribe();
                        let mut tick = tokio::time::interval(RETRY_INTERVAL);
                        tick.tick().await;
                        loop {
                            tokio::select! {
                                _ = active.cancelled() => {
                                    tracing::debug!("beacon reconciler stopping");
                                    break;
                                }
                                ev = rx.recv() => match ev {
                                    Ok(ev) if ev.kind == pistack_types::kind::ZONE => {
                                        if let Err(e) = client.reconcile_zone(&ev.id) {
                                            tracing::warn!(zone = %ev.id, error = %e, "beacon: reconcile failed");
                                        }
                                    }
                                    Ok(ev) if ev.kind == pistack_types::kind::RECORD => {
                                        let _ = client.reconcile_all();
                                    }
                                    Ok(_) => {}
                                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                        tracing::warn!(missed = n, "beacon reconciler lagged; full pass");
                                        let _ = client.reconcile_all();
                                    }
                                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                                },
                                _ = tick.tick() => {
                                    let _ = client.reconcile_all();
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

/// AXFR-pull every secondary zone; refresh the in-memory cache and status.
async fn transfer_secondaries(client: &Client, cache: &server::SecondaryCache) {
    let zones = match client.secondary_zones() {
        Ok(z) => z,
        Err(e) => {
            tracing::warn!(error = %e, "beacon: list secondaries");
            return;
        }
    };
    let mut keep = std::collections::HashSet::new();
    for z in zones {
        keep.insert(z.spec.name.clone());
        let ZoneKind::Secondary { primaries } = &z.spec.kind else {
            continue;
        };
        match xfer::pull_from_primaries(primaries, &z.spec.name).await {
            Ok((serial, data, from)) => {
                let records = data.record_count();
                let digest = format!("{from}:{serial}:{records}");
                cache.insert(data);
                if let Err(e) = client.secondary_ready(&z.meta.id, serial, records, digest, &from) {
                    tracing::warn!(zone = %z.spec.name, error = %e, "beacon: secondary Ready");
                } else {
                    tracing::info!(zone = %z.spec.name, serial, primary = %from, "beacon: secondary transferred");
                }
            }
            Err(e) => {
                tracing::warn!(zone = %z.spec.name, error = %e, "beacon: secondary transfer failed");
                let _ = client.secondary_failed(&z.meta.id, &e.to_string());
            }
        }
    }
    cache.retain_apices(&keep);
}
