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
pub mod bind;
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
    RouteView,
};
pub use types::*;

use async_trait::async_trait;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::sync::Arc;
use std::time::Duration;

/// Which providers the relay module mounts, chosen by `PISTACK_RELAY_BACKEND`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RelayBackend {
    /// Records balancers as ready without moving traffic. Tests and laptops.
    #[default]
    Mock,
    /// In-process L4 forwarder plus the HTTP reverse proxy.
    Forward,
}

impl RelayBackend {
    /// Unset or empty is `Mock`. Anything other than `mock` or `forward`
    /// (any case, surrounding spaces ignored) is an error, so a typo never
    /// leaves balancers on the mock without anyone noticing.
    pub fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.map(str::trim).map(str::to_ascii_lowercase).as_deref() {
            None | Some("") | Some("mock") => Ok(Self::Mock),
            Some("forward") => Ok(Self::Forward),
            Some(other) => Err(format!(
                "PISTACK_RELAY_BACKEND={other:?} is not a relay backend; use mock or forward"
            )),
        }
    }

    /// The backend for an explicit `PISTACK_RELAY_BACKEND` value and the
    /// `PISTACK_FABRIC_BACKEND` value. An explicit value always wins. When
    /// it is unset or empty, a server on a real datapath (`hostnat`,
    /// `overlay` or `ovn`) gets `Forward`, so its balancers move traffic; anything
    /// else keeps `Mock`.
    pub fn choose(value: Option<&str>, fabric: Option<&str>) -> Result<Self, String> {
        let explicit = value.map(str::trim).is_some_and(|v| !v.is_empty());
        if explicit {
            return Self::parse(value);
        }
        match fabric.map(str::trim) {
            Some("hostnat") | Some("overlay") | Some("ovn") => Ok(Self::Forward),
            _ => Ok(Self::Mock),
        }
    }

    /// Read `PISTACK_RELAY_BACKEND`, defaulting from `PISTACK_FABRIC_BACKEND`.
    pub fn from_env() -> Result<Self, String> {
        Self::choose(
            std::env::var("PISTACK_RELAY_BACKEND").ok().as_deref(),
            std::env::var("PISTACK_FABRIC_BACKEND").ok().as_deref(),
        )
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mock => "mock",
            Self::Forward => "forward",
        }
    }
}

/// How often the reconciler retries resources that are not `Ready`.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// The module. `Default` uses the mock provider so a single-node server is
/// functional end to end. [`Relay::forward`] is the in-process L4 forwarder
/// and the HTTP reverse proxy (`PISTACK_RELAY_BACKEND=forward`, or the
/// default when the fabric backend is `hostnat`, `overlay` or `ovn`).
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

#[cfg(test)]
mod backend_tests {
    use super::*;

    #[test]
    fn backend_choice_is_explicit() {
        assert_eq!(RelayBackend::parse(None), Ok(RelayBackend::Mock));
        assert_eq!(RelayBackend::parse(Some("")), Ok(RelayBackend::Mock));
        assert_eq!(RelayBackend::parse(Some("mock")), Ok(RelayBackend::Mock));
        assert_eq!(
            RelayBackend::parse(Some("forward")),
            Ok(RelayBackend::Forward)
        );
        assert_eq!(
            RelayBackend::parse(Some(" Forward\n")),
            Ok(RelayBackend::Forward)
        );
        assert!(RelayBackend::parse(Some("fwd")).is_err());
        assert!(RelayBackend::parse(Some("ovn")).is_err());
    }

    #[test]
    fn a_real_fabric_defaults_to_forward() {
        use RelayBackend::{Forward, Mock};
        assert_eq!(RelayBackend::choose(None, None), Ok(Mock));
        assert_eq!(RelayBackend::choose(None, Some("mock")), Ok(Mock));
        assert_eq!(RelayBackend::choose(None, Some("hostnat")), Ok(Forward));
        assert_eq!(RelayBackend::choose(Some(""), Some("overlay")), Ok(Forward));
        assert_eq!(RelayBackend::choose(None, Some("ovn")), Ok(Forward));
        assert_eq!(
            RelayBackend::choose(Some(" "), Some("overlay")),
            Ok(Forward)
        );
        // An explicit value always wins, including a typo.
        assert_eq!(
            RelayBackend::choose(Some("mock"), Some("hostnat")),
            Ok(Mock)
        );
        assert_eq!(RelayBackend::choose(Some("forward"), None), Ok(Forward));
        assert!(RelayBackend::choose(Some("fwd"), Some("hostnat")).is_err());
    }

    #[test]
    fn forward_backend_serves_l4_and_http() {
        let relay = Relay::for_backend(RelayBackend::Forward);
        let names = relay.providers.names();
        assert!(names.contains(&ForwardProvider::new().name()), "{names:?}");
        assert!(!names.contains(&"mock"), "{names:?}");
    }

    #[test]
    fn old_ovn_rows_read_as_forward() {
        let p: Provider = serde_json::from_str("\"ovn\"").unwrap();
        assert_eq!(p, Provider::Forward);
        assert_eq!(serde_json::to_string(&p).unwrap(), "\"forward\"");
        assert!(!OvnProvider::default().serves(Provider::Forward));
    }
}
