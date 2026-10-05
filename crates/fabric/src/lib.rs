// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `fabric`: networks, subnets, ports, routers, security groups and
//! floating IPs. Fabric realises networks through a backend: the overlay
//! backend is a VXLAN segment per network and a network namespace per
//! router; host NAT remains the single-host datapath; the mock is for tests.
//!
//! Layout:
//! * [`types`] — the resource model.
//! * [`ipam`] — deterministic IP and MAC allocation.
//! * [`overlay`] — the per-host VXLAN plan. It does not change the host.
//! * [`Client`] — validation and writes; the library API other modules use.
//! * `reconcile` — turns spec into backend state and writes status.
//! * `machine` — one automatic floating IP per machine IPv4.
//! * `api` — HTTP, mounted at `/network`.
//!
//! Other modules never touch fabric's collections directly; they call
//! [`Client::create_port_for_device`] and [`Client::release_device_ports`].

pub mod api;
pub mod backend;
pub mod client;
pub mod ipam;
mod machine;
pub mod overlay;
mod reconcile;
pub mod types;
mod usage;

pub use backend::{HostNat, MockBackend, NetBackend, OverlayBackend, OvnBackend};
pub use client::{Client, DEFAULT_SG};
pub use overlay::{
    device_hex, plan, OverlayPlan, PlanError, PlanHost, PlanNetwork, PlanPort, PlanRouter,
};
pub use reconcile::Observed;
pub use types::*;

use async_trait::async_trait;
use pistack_store::Store;
use pistack_types::{kind, ApiResult, Id, Module, ModuleContext, ModuleInfo};
use std::sync::Arc;
use std::time::Duration;

/// How often the reconciler retries resources that are not `Ready`.
const RETRY_INTERVAL: Duration = Duration::from_secs(30);

/// The module. `Default` uses the mock backend so a single-node server is
/// functional end to end. [`Fabric::hostnat`] is the single-host datapath.
/// [`Fabric::overlay`] accepts overlay networks; the agent applies the plan.
pub struct Fabric {
    backend: Arc<dyn NetBackend>,
}

impl Default for Fabric {
    fn default() -> Self {
        Self::with_backend(Arc::new(MockBackend::new()))
    }
}

impl Fabric {
    pub fn with_backend(backend: Arc<dyn NetBackend>) -> Self {
        Self { backend }
    }

    /// Floating IPs are 1:1 NAT on the host WireGuard interface.
    /// Selected when `PISTACK_FABRIC_BACKEND=hostnat`.
    pub fn hostnat() -> Self {
        Self::with_backend(Arc::new(HostNat::new()))
    }

    /// Overlay networks: one VXLAN segment per network, one network namespace
    /// per router. The agent applies [`overlay::OverlayPlan`].
    pub fn overlay() -> Self {
        Self::with_backend(Arc::new(OverlayBackend::new()))
    }

    /// A `Client` over the context's store and this module's backend.
    pub fn client(&self, ctx: &ModuleContext) -> Client {
        let store = ctx.store::<Store>();
        Client::new((*store).clone(), self.backend.clone())
    }
}

/// The project's "default" security group, cloned from the cloud-wide
/// template on first use. Convenience over [`Client::default_security_group`]
/// for callers that only hold a store.
pub fn default_security_group(store: &Store, project: &Id) -> ApiResult<SecurityGroup> {
    Client::new(store.clone(), Arc::new(MockBackend::new())).default_security_group(project)
}

#[async_trait]
impl Module for Fabric {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "fabric",
            prefix: "network",
            summary: "Networks, subnets, routers, security groups, floating IPs",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        api::router(self.client(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client(&ctx);
        client.ensure_default_template()?;
        tracing::info!(backend = self.backend.name(), "fabric: initial reconcile");
        client.reconcile_all().await;
        run_auto_fips(&client);

        let mut rx = ctx.events.subscribe();
        let shutdown = ctx.shutdown.clone();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(RETRY_INTERVAL);
            tick.tick().await; // the first tick fires immediately; we just did a pass
            loop {
                tokio::select! {
                    _ = shutdown.cancelled() => {
                        tracing::debug!("fabric reconciler stopping");
                        break;
                    }
                    ev = rx.recv() => match ev {
                        Ok(ev) => {
                            if ev.kind == kind::MACHINE {
                                run_auto_fips(&client);
                            }
                            client.handle_event(&ev).await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                            tracing::warn!(missed = n, "fabric reconciler lagged; full pass");
                            run_auto_fips(&client);
                            client.reconcile_pending().await;
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    },
                    _ = tick.tick() => {
                        run_auto_fips(&client);
                        client.reconcile_pending().await;
                    },
                }
            }
        });
        Ok(())
    }
}

fn run_auto_fips(client: &Client) {
    if let Err(e) = client.sync_auto_floating_ips() {
        tracing::warn!(error = %e, "fabric auto-fip pass failed");
    }
}
