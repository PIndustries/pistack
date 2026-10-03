// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `beacon`: DNS zones and record sets, served by an embedded authoritative
//! server (hickory). Replaces Designate and CoreDNS.
//!
//! Layout:
//! * [`types`] — `Zone`, `RecordSet`, validation and zone-file export.
//! * [`Client`] — validation and writes; the library API other modules use
//!   ([`Client::upsert_record`], [`Client::remove_record`]).
//! * [`server`] — the hickory request handler and UDP/TCP listener.
//! * [`auto`] — `<machine>.<zone>` records for zones labelled
//!   `pistack.io/auto=true`.
//! * `api` — HTTP, mounted at `/dns`.
//!
//! The serial reconciler runs in [`Module::start`]: every zone or record
//! change recomputes the zone's content digest and bumps `status.serial`
//! when it moved, which in turn refreshes the server's cache.

pub mod api;
pub mod auto;
pub mod client;
pub mod server;
pub mod types;

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

/// The module. `Default` reads the listen address from `PISTACK_DNS_LISTEN`
/// (default `0.0.0.0:5353`); tests use [`Beacon::with_listen`] and port 0.
#[derive(Default)]
pub struct Beacon {
    listen: Option<SocketAddr>,
    bound: Arc<OnceLock<SocketAddr>>,
}

impl Beacon {
    pub fn with_listen(addr: SocketAddr) -> Self {
        Self {
            listen: Some(addr),
            bound: Arc::new(OnceLock::new()),
        }
    }

    /// A `Client` over the context's store.
    pub fn client(&self, ctx: &ModuleContext) -> Client {
        let store = ctx.store::<Store>();
        Client::new((*store).clone())
    }

    /// Address the DNS server bound to, once `start` has run.
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
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        api::router(self.client(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client(&ctx);
        client.reconcile_all()?;

        let listen = match self.listen {
            Some(a) => a,
            None => server::listen_addr()?,
        };
        let bound = server::serve(
            client.clone(),
            listen,
            ctx.events.clone(),
            ctx.shutdown.clone(),
        )
        .await?;
        let _ = self.bound.set(bound);

        auto::start(client.clone(), ctx.events.subscribe(), ctx.shutdown.clone());

        let mut rx = ctx.events.subscribe();
        let shutdown = ctx.shutdown.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(RETRY_INTERVAL);
            tick.tick().await; // the first tick fires immediately; we just did a pass
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::debug!("beacon reconciler stopping");
                        break;
                    }
                    ev = rx.recv() => match ev {
                        Ok(ev) if ev.kind == pistack_types::kind::ZONE => {
                            if let Err(e) = client.reconcile_zone(&ev.id) {
                                tracing::warn!(zone = %ev.id, error = %e, "beacon: reconcile failed");
                            }
                        }
                        // A record event carries no zone; a Deleted one has no
                        // body to look up either. A full pass is cheap.
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
        });
        Ok(())
    }
}
