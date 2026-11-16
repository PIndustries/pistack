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
//! * [`notify`] — NOTIFY (RFC 1996) in both directions: tell secondaries a
//!   serial moved, and pull at once when a primary tells us.
//! * [`auto`] — `<machine>.<zone>` records for zones labelled
//!   `pistack.io/auto=true`.
//! * `api` — HTTP, mounted at `/dns`.
//!
//! The serial reconciler, the auto-record writer and the NOTIFY sender run
//! only while this node is the cluster leader. The authoritative DNS listener
//! and the NOTIFY receiver stay up on every node so local lookups keep working
//! from a replicated store and a NOTIFY arriving at any node is acted on.

pub mod api;
pub mod auto;
pub mod client;
pub mod notify;
pub mod server;
pub mod types;
pub mod xfer;

pub use client::Client;
pub use notify::Notifier;
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
    /// Set by `start`; how a zone write asks for a NOTIFY. `None` before that,
    /// and for a router built without a listener.
    notifier: Arc<OnceLock<Notifier>>,
}

impl Beacon {
    pub fn with_listen(addr: SocketAddr) -> Self {
        Self {
            listen: Some(addr),
            cluster_hint: None,
            bound: Arc::new(OnceLock::new()),
            notifier: Arc::new(OnceLock::new()),
        }
    }

    /// Prefer the private address in `server_listen` when the env is unset.
    pub fn with_cluster_hint(server_listen: impl Into<String>) -> Self {
        Self {
            listen: None,
            cluster_hint: Some(server_listen.into()),
            bound: Arc::new(OnceLock::new()),
            notifier: Arc::new(OnceLock::new()),
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

    /// Ask for `apex` to be notified to its secondaries. A no-op before
    /// `start`, when there is no notify loop to answer.
    pub fn notify_zone(&self, apex: &str) {
        if let Some(n) = self.notifier.get() {
            n.notify_zone(apex);
        }
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
        api::router(self.client(&ctx), self.notifier.get().cloned())
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client(&ctx);
        let secondaries = server::SecondaryCache::new();
        let (notifier, forced_rx) = Notifier::new();
        let forced = Arc::new(parking_lot::Mutex::new(Some(forced_rx)));
        let _ = self.notifier.set(notifier);
        let (pulled_tx, pulled_rx) = tokio::sync::mpsc::unbounded_channel();

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
            Some(pulled_tx),
        )
        .await?;
        if let Some(first) = bounds.first().copied() {
            let _ = self.bound.set(first);
        }
        // An accepted NOTIFY is pulled on the node that accepted it, leader or
        // not, so the pull never waits on a leadership handover.
        {
            let client = client.clone();
            let secondaries = secondaries.clone();
            let shutdown = ctx.shutdown.clone();
            tokio::spawn(notify::run_pulls(client, secondaries, pulled_rx, shutdown));
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
                    let forced = Arc::clone(&forced);
                    async move {
                        if let Err(e) = client.reconcile_all() {
                            pistack_types::tally::reconcile_failed("beacon");
                            tracing::warn!(error = %e, "beacon: initial reconcile");
                        }
                        auto::start(
                            client.clone(),
                            events.subscribe(),
                            active.clone(),
                        );
                        // The queue belongs to whoever holds leadership, so it
                        // travels with this run and comes back when it ends.
                        if let Some(queue) = forced.lock().take() {
                            let nc = client.clone();
                            let ne = events.subscribe();
                            let na = active.clone();
                            tokio::spawn(notify::run_notifies(nc, ne, queue, na));
                        }

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
                                            pistack_types::tally::reconcile_failed("beacon");
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
    for z in &zones {
        keep.insert(z.spec.name.clone());
    }
    for z in zones {
        transfer_one(client, cache, &z.spec.name, None).await;
    }
    cache.retain_apices(&keep);
}

/// AXFR-pull one secondary zone, named by apex, from `from` first when it is
/// one of the zone's primaries. `from` is the sender of an accepted NOTIFY;
/// its data is the change we were told about.
async fn transfer_one(
    client: &Client,
    cache: &server::SecondaryCache,
    apex: &str,
    from: Option<std::net::IpAddr>,
) {
    let zones = match client.secondary_zones() {
        Ok(z) => z,
        Err(e) => {
            tracing::warn!(error = %e, "beacon: list secondaries");
            return;
        }
    };
    let Some(z) = zones
        .into_iter()
        .find(|z| notify::same_apex(&z.spec.name, apex))
    else {
        tracing::debug!(zone = apex, "beacon: transfer for a zone we do not hold");
        return;
    };
    let ZoneKind::Secondary { primaries } = &z.spec.kind else {
        return;
    };
    let listed: Vec<String> = match from {
        Some(ip) => primaries
            .iter()
            .filter(|p| notify::parse_addr(p, notify::DNS_PORT).ok().map(|a| a.ip()) == Some(ip))
            .cloned()
            .collect(),
        None => Vec::new(),
    };
    let pulled = if listed.is_empty() {
        xfer::pull_from_primaries(primaries, &z.spec.name).await
    } else {
        xfer::pull_from_listed_first(&listed, primaries, &z.spec.name).await
    };
    match pulled {
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
