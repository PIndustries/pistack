// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Container orchestration: pods, deployments, services, controllers.
//!
//! * [`types`] — `Pod`, `ReplicaSet`, `Deployment`, `StatefulSet`,
//!   `DaemonSet`, `Service`, `ConfigMap`, `Job`, `CronJob`, `Ingress`;
//!   all `Resource<Spec, Status>` (metadata + spec + status, labels,
//!   selectors).
//! * [`controllers`] — one reconcile loop per kind. Pods become forge
//!   `Machine`s of kind `Container`; see [`controllers::pod`] for the
//!   machine → anvil contract (`PodSandbox` in `user_data`).
//! * [`selector`] — label selectors and the `pod-template-hash`.
//! * [`cron`] — the five-field cron parser CronJobs use.
//! * [`realizer`] — `ServiceRealizer`, the seam to fabric/relay.
//! * [`http`] — the `loom` API: `/pods`, `/replicasets`, `/deployments`,
//!   `/statefulsets`, `/daemonsets`, `/services`, `/configmaps`, `/jobs`,
//!   `/cronjobs`, `/ingresses`.
//!
//! `Loom` is dev mode: unauthenticated requests are admin, services are
//! realised by [`NoopRealizer`], cluster IPs come from `10.96.0.0/12`.
//! Production wiring builds a [`LoomConfig`] and mounts
//! [`Loom::configured`]. [`LoomConfig::production`] programs service and
//! ingress listeners through relay and reads pod logs and runs pod exec
//! through the command queue.

pub mod controllers;
pub mod cron;
pub mod http;
pub mod realizer;
pub mod selector;
pub mod types;

pub use controllers::pod::{PodSandbox, EXITED_CONDITION, SANDBOX_API_VERSION, SANDBOX_KIND};
pub use cron::{CronError, Schedule};
pub use realizer::{NoopRealizer, RelayRealizer, ServiceRealizer};
pub use types::*;

use async_trait::async_trait;
use axum::Router;
use controllers::{
    cronjob::CronJobController,
    daemonset::DaemonSetController,
    deployment::DeploymentController,
    ingress::IngressController,
    job::JobController,
    pod::PodController,
    replicaset::ReplicaSetController,
    service::{ServiceController, DEFAULT_SERVICE_CIDR},
    statefulset::StatefulSetController,
};
use ipnet::IpNet;
use pistack_rpc::CommandQueue;
use pistack_store::Store;
use pistack_types::{Module, ModuleContext, ModuleInfo};
use std::sync::Arc;

/// Runtime configuration.
#[derive(Clone)]
pub struct LoomConfig {
    /// Treat unauthenticated requests as cloud admin.
    pub dev_admin: bool,
    /// Where Service cluster IPs come from.
    pub service_cidr: IpNet,
    /// What makes cluster IPs route.
    pub realizer: Arc<dyn ServiceRealizer>,
    /// Service and ingress listeners. `None` in dev mode.
    pub relay: Option<Arc<RelayRealizer>>,
    /// Pod log reads and pod exec. `None` until the server supplies the command queue.
    pub queue: Option<CommandQueue>,
}

impl Default for LoomConfig {
    fn default() -> Self {
        Self {
            dev_admin: true,
            service_cidr: DEFAULT_SERVICE_CIDR
                .parse()
                .unwrap_or(IpNet::V4(ipnet::Ipv4Net::default())),
            realizer: Arc::new(NoopRealizer),
            relay: None,
            queue: None,
        }
    }
}

impl LoomConfig {
    /// Relay listeners for services and ingresses, and the command queue
    /// pod logs and pod exec use. `dev_admin` matches the server flag.
    pub fn production(dev_admin: bool, queue: CommandQueue) -> Self {
        let relay = Arc::new(RelayRealizer::new());
        Self {
            dev_admin,
            service_cidr: DEFAULT_SERVICE_CIDR
                .parse()
                .unwrap_or(IpNet::V4(ipnet::Ipv4Net::default())),
            realizer: relay.clone(),
            relay: Some(relay),
            queue: Some(queue),
        }
    }
}

impl LoomConfig {
    pub fn dev_admin(mut self, on: bool) -> Self {
        self.dev_admin = on;
        self
    }
    pub fn service_cidr(mut self, cidr: IpNet) -> Self {
        self.service_cidr = cidr;
        self
    }
    pub fn realizer(mut self, r: Arc<dyn ServiceRealizer>) -> Self {
        self.realizer = r;
        self
    }
}

/// The module in dev mode (see [`LoomConfig::default`]).
#[derive(Default)]
pub struct Loom;

impl Loom {
    /// The module with explicit configuration.
    pub fn configured(config: LoomConfig) -> ConfiguredLoom {
        ConfiguredLoom { config }
    }
}

/// The module with a [`LoomConfig`].
pub struct ConfiguredLoom {
    config: LoomConfig,
}

impl ConfiguredLoom {
    pub fn config(&self) -> &LoomConfig {
        &self.config
    }
}

fn info() -> ModuleInfo {
    ModuleInfo {
        name: "loom",
        prefix: "loom",
        summary: "Container orchestration: pods, deployments, services, controllers",
        version: env!("CARGO_PKG_VERSION"),
    }
}

fn router(config: &LoomConfig, ctx: ModuleContext) -> Router {
    let store = ctx.store::<Store>();
    http::router(http::AppState {
        store: (*store).clone(),
        dev_admin: config.dev_admin,
        queue: config.queue.clone(),
    })
}

async fn start(config: &LoomConfig, ctx: ModuleContext) -> anyhow::Result<()> {
    let store = (*ctx.store::<Store>()).clone();
    if let Some(relay) = &config.relay {
        relay.bind_store(store.clone());
    }
    tokio::spawn(controllers::run(
        PodController::new(store.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        ReplicaSetController::new(store.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        DeploymentController::new(store.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        StatefulSetController::new(store.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        DaemonSetController::new(store.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        ServiceController::new(store.clone(), config.service_cidr, config.realizer.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        IngressController::new(store.clone(), config.relay.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(
        JobController::new(store.clone()),
        ctx.clone(),
    ));
    tokio::spawn(controllers::run(CronJobController::new(store), ctx));
    tracing::info!("loom: controllers started");
    Ok(())
}

#[async_trait]
impl Module for Loom {
    fn info(&self) -> ModuleInfo {
        info()
    }
    fn router(&self, ctx: ModuleContext) -> Router {
        router(&LoomConfig::default(), ctx)
    }
    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        start(&LoomConfig::default(), ctx).await
    }
}

#[async_trait]
impl Module for ConfiguredLoom {
    fn info(&self) -> ModuleInfo {
        info()
    }
    fn router(&self, ctx: ModuleContext) -> Router {
        router(&self.config, ctx)
    }
    async fn start(&self, ctx: ModuleContext) -> anyhow::Result<()> {
        start(&self.config, ctx).await
    }
}
