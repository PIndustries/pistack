// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Machines: VMs, containers and bare metal as one resource.
//!
//! * [`types`] — `Host`, `Size`, `Machine`.
//! * [`scheduler`] — pure placement function.
//! * [`driver`] — `HostDriver` trait, `MockDriver`, `ChannelDriver`,
//!   `DriverRegistry`: the seam to `anvil` (in-process or over RPC).
//! * [`reconcile`] — the loop that turns `spec` into `status`.
//! * [`http`] — the `compute` API.
//!
//! `Forge::default()` is dev mode: a single `local` host backed by a
//! `MockDriver` is registered at start so `pi machine create` works on one
//! box with no agent. Production wiring builds a `DriverRegistry`, hands it
//! to [`Forge::with_registry`], and registers a driver per host as agents
//! say hello.

pub mod driver;
pub mod http;
pub mod reconcile;
pub mod scheduler;
pub mod types;

pub use driver::{
    ChannelDriver, DriverCommand, DriverRegistry, DriverReply, DriverRequest, HostDriver,
    MachineRuntime, MockDriver,
};
pub use reconcile::Reconciler;
pub use scheduler::{place, Allocations, Strategy};
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use pistack_store::{Store, StoreError};
use pistack_types::{Module, ModuleContext, ModuleInfo, Phase, Resources};
use std::collections::BTreeMap;
use std::sync::Arc;

/// Name of the auto-registered dev host.
pub const LOCAL_HOST_NAME: &str = "local";

pub struct Forge {
    drivers: Arc<DriverRegistry>,
    /// Register a `local` host with a `MockDriver` at start.
    local_host: bool,
    /// Treat unauthenticated requests as cloud admin.
    dev_admin: bool,
    strategy: Strategy,
}

impl Default for Forge {
    /// Dev mode: local mock host, unauthenticated requests are admin.
    fn default() -> Self {
        Self {
            drivers: Arc::new(DriverRegistry::new()),
            local_host: true,
            dev_admin: true,
            strategy: Strategy::Spread,
        }
    }
}

impl Forge {
    /// Production shape: drivers come from `registry`, nothing is
    /// auto-registered and requests need a `Caller`.
    pub fn with_registry(registry: Arc<DriverRegistry>) -> Self {
        Self {
            drivers: registry,
            local_host: false,
            dev_admin: false,
            strategy: Strategy::Spread,
        }
    }

    pub fn local_host(mut self, on: bool) -> Self {
        self.local_host = on;
        self
    }
    pub fn dev_admin(mut self, on: bool) -> Self {
        self.dev_admin = on;
        self
    }
    pub fn strategy(mut self, s: Strategy) -> Self {
        self.strategy = s;
        self
    }
    /// The registry the reconciler consults. Register per-host drivers here.
    pub fn drivers(&self) -> Arc<DriverRegistry> {
        self.drivers.clone()
    }

    /// Create the default sizes that do not exist yet.
    pub fn seed_sizes(store: &Store) -> anyhow::Result<usize> {
        let col = store.collection::<Size>();
        let mut n = 0;
        for s in default_sizes() {
            match col.create(s) {
                Ok(_) => n += 1,
                Err(StoreError::Conflict(_)) => {}
                Err(e) => return Err(e.into()),
            }
        }
        Ok(n)
    }

    /// Ensure the `local` host exists and is served by a `MockDriver`.
    pub fn ensure_local_host(&self, store: &Store, node_name: &str) -> anyhow::Result<Host> {
        let col = store.collection::<Host>();
        let host = match col.get_by_name(None, LOCAL_HOST_NAME) {
            Ok(h) => h,
            Err(StoreError::NotFound(_)) => {
                let mut labels = BTreeMap::new();
                labels.insert("pistack.io/local".into(), "true".into());
                col.create(new_host(
                    LOCAL_HOST_NAME,
                    node_name,
                    local_capacity(),
                    vec!["mock".into()],
                    labels,
                ))?
            }
            Err(e) => return Err(e.into()),
        };
        let host = if host.status.phase != Phase::Ready {
            col.modify(&host.meta.id, |h| {
                h.status.phase = Phase::Ready;
                h.status.message.clear();
                h.status.last_seen = Some(chrono::Utc::now());
                Ok(())
            })?
        } else {
            host
        };
        if self.drivers.get(&host.meta.id).is_none() {
            self.drivers
                .register_local(host.meta.id.clone(), Arc::new(MockDriver::new()));
        }
        Ok(host)
    }
}

/// What the dev host advertises: this box's cores, its memory from
/// /proc/meminfo when readable, and a nominal disk.
fn local_capacity() -> Resources {
    let vcpus = std::thread::available_parallelism()
        .map(|n| n.get() as u32)
        .unwrap_or(4);
    let memory_mib = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|s| {
            s.lines()
                .find(|l| l.starts_with("MemTotal:"))
                .and_then(|l| l.split_whitespace().nth(1))
                .and_then(|kb| kb.parse::<u64>().ok())
                .map(|kb| kb / 1024)
        })
        .unwrap_or(8192);
    Resources {
        vcpus,
        memory_mib,
        disk_gib: 100,
        gpus: 0,
    }
}

#[async_trait]
impl Module for Forge {
    fn info(&self) -> ModuleInfo {
        ModuleInfo {
            name: "forge",
            prefix: "compute",
            summary: "Machines: VMs, containers and bare metal as one resource",
            version: env!("CARGO_PKG_VERSION"),
        }
    }

    fn router(&self, ctx: ModuleContext) -> Router {
        let store = ctx.store::<Store>();
        http::router(http::AppState {
            store: (*store).clone(),
            drivers: self.drivers.clone(),
            dev_admin: self.dev_admin,
        })
    }

    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        let store = (*ctx.store::<Store>()).clone();
        let seeded = Self::seed_sizes(&store)?;
        if seeded > 0 {
            tracing::info!(count = seeded, "forge: seeded default sizes");
        }
        if self.local_host {
            let h = self.ensure_local_host(&store, &ctx.node_name)?;
            tracing::info!(host = %h.meta.id, capacity = ?h.spec.capacity, "forge: dev mode, local mock host registered");
        }
        let reconciler = Reconciler::new(store, self.drivers.clone(), self.strategy);
        tokio::spawn(reconciler.run(ctx));
        Ok(())
    }
}
