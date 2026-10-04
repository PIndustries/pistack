// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `relay`: load balancers. L4 is an in-process TCP forwarder. HTTP is an
//! in-process reverse proxy. Both sit behind [`provider::LbProvider`].
//!
//! Layout:
//! * [`types`] — `Balancer`, `Listener`, `Pool`, `Member`.
//! * [`Client`] — validation and writes; the library API other modules use
//!   ([`Client::ensure_tcp_balancer`] for `loom` services).
//! * [`provider`] — the `LbProvider` trait, `MockProvider`, `ForwardProvider`,
//!   and the HTTP reverse proxy.
//! * [`door`] — publish a listener. Cloudflared and tailscale run when the
//!   binary is present. WireGuard creates only a `pw` interface and is
//!   unavailable when `wg` is missing. Edge queues a publish for a host
//!   whose driver is `edge` and starts no process.
//! * `reconcile` — turns spec into provider state and writes status.
//! * [`health`] — active TCP/HTTP checks driving `Member.operating_status`.
//! * `api` — HTTP, mounted at `/balancers`.

pub mod api;
mod bind;
pub mod client;
pub mod door;
pub mod health;
pub mod provider;
mod reconcile;
mod tls;
pub mod types;
mod udp;

pub use client::Client;
pub use door::{DoorKind, DoorPlan};
pub use provider::{
    Call, ForwardProvider, LbProvider, MockProvider, OvnProvider, Providers, RelayL7Provider,
};
pub use types::*;

use async_trait::async_trait;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::sync::Arc;
use std::time::Duration;

/// How often the reconciler retries resources that are not `Ready`.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// The module. `Default` uses the mock provider so a single-node server is
/// functional end to end. [`Relay::forward`] is the in-process L4 forwarder
/// and the HTTP reverse proxy (`PISTACK_RELAY_BACKEND=forward`).
pub struct Relay {
    providers: Providers,
    queue: Option<CommandQueue>,
}

impl Default for Relay {
    fn default() -> Self {
        Self::with_providers(vec![Arc::new(MockProvider::new())])
    }
}

impl Relay {
    pub fn with_providers(list: Vec<Arc<dyn LbProvider>>) -> Self {
        Self {
            providers: Providers::new(list),
            queue: None,
        }
    }

    /// Edge publishes are queued on this agent command queue.
    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    /// In-process L4 TCP forwarder and HTTP reverse proxy.
    /// Selected when `PISTACK_RELAY_BACKEND=forward`.
    pub fn forward() -> Self {
        Self::with_providers(vec![
            Arc::new(ForwardProvider::new()),
            Arc::new(RelayL7Provider::new()),
        ])
    }

    /// A `Client` over the context's store and this module's providers.
    pub fn client(&self, ctx: &ModuleContext) -> Client {
        let store = ctx.store::<Store>();
        let mut client = Client::new((*store).clone(), self.providers.clone());
        if let Some(queue) = &self.queue {
            client = client.with_queue(queue.clone());
        }
        client
    }
}

#[async_trait]
impl Module for Relay {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "relay",
            prefix: "balancers",
            summary: "Load balancers",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        api::router(self.client(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client(&ctx);
        tracing::info!(providers = ?self.providers.names(), "relay: initial reconcile");
        client.reconcile_all().await;

        health::start(client.clone(), ctx.shutdown.clone());

        let mut rx = ctx.events.subscribe();
        let shutdown = ctx.shutdown.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(RETRY_INTERVAL);
            tick.tick().await; // the first tick fires immediately; we just did a pass
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::debug!("relay reconciler stopping");
                        break;
                    }
                    ev = rx.recv() => match ev {
                        Ok(ev) if KINDS.contains(&ev.kind.as_str()) => client.handle_event(&ev).await,
                        Ok(_) => {}
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(missed = n, "relay reconciler lagged; full pass");
                            client.reconcile_pending().await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    _ = tick.tick() => client.reconcile_pending().await,
                }
            }
        });
        Ok(())
    }
}
