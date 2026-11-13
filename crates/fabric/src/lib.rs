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
//! * `machine` — automatic floating IPs, for a network marked to give them.
//! * `api` — HTTP, mounted at `/network`.
//!
//! Other modules never touch fabric's collections directly; they call
//! [`Client::create_port_for_device`] and [`Client::release_device_ports`].
//!
//! The reconciler and auto-floating-IP loop run only while this node is the
//! cluster leader. Followers stay idle so they never write a local copy.

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
/// [`Fabric::ovn`] writes OVN's northbound database.
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

    /// OVN: the leader writes OVN's northbound database at
    /// `PISTACK_OVN_NB` (default [`backend::ovn::DEFAULT_NB`]); OVS and
    /// `ovn-controller` on each host do the rest.
    /// Selected when `PISTACK_FABRIC_BACKEND=ovn`.
    pub fn ovn() -> Self {
        Self::with_backend(Arc::new(OvnBackend::from_env()))
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
            version: pistack_types::BUILD,
        }
    }

    fn router(&self, ctx: ModuleContext) -> axum::Router {
        api::router(self.client(&ctx))
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let client = self.client(&ctx);
        self.backend.attach(&ctx.leadership, &ctx.store::<Store>());
        let backend_name = self.backend.name();
        let leadership = ctx.leadership.clone();
        let shutdown = ctx.shutdown.clone();
        let events = ctx.events.clone();
        tokio::spawn(async move {
            leadership
                .run_while_leader(shutdown, move |active| {
                    let client = client.clone();
                    let events = events.clone();
                    let backend_name = backend_name;
                    async move {
                        if let Err(e) = client.ensure_default_template() {
                            tracing::warn!(error = %e, "fabric: default template");
                        }
                        tracing::info!(backend = backend_name, "fabric: initial reconcile");
                        client.reconcile_all().await;
                        note_unmarked_auto_fips(&client);
                        run_auto_fips(&client);

                        let mut rx = events.subscribe();
                        let mut tick = tokio::time::interval(RETRY_INTERVAL);
                        tick.tick().await; // the first tick fires immediately; we just did a pass
                        loop {
                            tokio::select! {
                                _ = active.cancelled() => {
                                    tracing::debug!("fabric reconciler stopping");
                                    break;
                                }
                                ev = rx.recv() => match ev {
                                    Ok(ev) => {
                                        // A network event may be the mark going on.
                                        if ev.kind == kind::MACHINE || ev.kind == kind::NETWORK {
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
                                    if let Err(e) = client.backend().refresh().await {
                                        tracing::warn!(error = %e.message, "fabric: backend refresh");
                                    }
                                },
                            }
                        }
                    }
                })
                .await;
        });
        Ok(())
    }
}

/// Say once per term that floating IPs handed out before networks had to
/// be marked are still there.
fn note_unmarked_auto_fips(client: &Client) {
    match client.auto_fips_without_a_network() {
        Ok(0) => {}
        Ok(count) => tracing::warn!(
            count,
            "fabric: automatic floating IPs exist but no external network has \
             auto_floating_ip set. They stay in place and no new ones are created. \
             Mark the network with `pi network set <name> --auto-floating-ip true`, \
             or delete the ones you do not want."
        ),
        Err(e) => tracing::warn!(error = %e, "fabric: could not count automatic floating IPs"),
    }
}

fn run_auto_fips(client: &Client) {
    if let Err(e) = client.sync_auto_floating_ips() {
        tracing::warn!(error = %e, "fabric auto-fip pass failed");
    }
}
