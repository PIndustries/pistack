// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The module a server mounts: which providers it carries, the `Client` it
//! hands the api, and the reconciler and health checker it starts as the
//! leader.

use crate::api;
use crate::backend::RelayBackend;
use crate::client::Client;
use crate::health;
use crate::provider::{ForwardProvider, LbProvider, MockProvider, Providers, RelayL7Provider};
use crate::relay::retry_interval::RETRY_INTERVAL;
use crate::types::KINDS;
use async_trait::async_trait;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::sync::Arc;

/// The module. `Default` uses the mock provider so a single-node server is
/// functional end to end. [`Relay::forward`] is the in-process L4 forwarder
/// and the HTTP reverse proxy (`PISTACK_RELAY_BACKEND=forward`, or the
/// default when the fabric backend is `hostnat`, `overlay` or `ovn`).
pub struct Relay {
    pub(crate) providers: Providers,
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

    /// The module for `backend`.
    pub fn for_backend(backend: RelayBackend) -> Self {
        match backend {
            RelayBackend::Mock => Self::default(),
            RelayBackend::Forward => Self::forward(),
        }
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
            version: pistack_types::BUILD,
        }
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        api::router(self.client(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        // Loom programs its own balancers on its own providers; binding
        // them here as well fails with "address already in use".
        let client = self.client(&ctx).leave_loom_managed();
        let provider_names = self.providers.names();
        let leadership = ctx.leadership.clone();
        let shutdown = ctx.shutdown.clone();
        let events = ctx.events.clone();
        tokio::spawn(async move {
            leadership
                .run_while_leader(shutdown, move |active| {
                    let client = client.clone();
                    let events = events.clone();
                    let provider_names = provider_names.clone();
                    async move {
                        tracing::info!(providers = ?provider_names, "relay: initial reconcile");
                        client.reconcile_all().await;
                        health::start(client.clone(), active.clone());

                        let mut rx = events.subscribe();
                        let mut tick = tokio::time::interval(RETRY_INTERVAL);
                        tick.tick().await;
                        loop {
                            tokio::select! {
                                _ = active.cancelled() => {
                                    tracing::debug!("relay reconciler stopping");
                                    break;
                                }
                                ev = rx.recv() => match ev {
                                    Ok(ev) if KINDS.contains(&ev.kind.as_str()) => {
                                        client.handle_event(&ev).await
                                    }
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
                    }
                })
                .await;
        });
        Ok(())
    }
}
