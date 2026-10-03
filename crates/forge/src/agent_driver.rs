// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The bridge to real hosts over `pistack-rpc`.
//!
//! * [`RpcDriver`] is a [`HostDriver`] that turns every call into a command
//!   on the [`CommandQueue`] for one node and awaits the agent's result.
//! * [`Registrar`] listens to the queue's agent events: a hello creates or
//!   refreshes the `Host` resource for that node and registers an
//!   `RpcDriver` for it; a heartbeat bumps `last_seen` and folds the agent's
//!   machine reports (addresses, power) into machine status.
//!
//! Wire shapes are `pistack_rpc::proto::{MachineRequest, MachineRef,
//! RunningMachine}`, shared with `anvil`.

use std::collections::BTreeMap;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use pistack_rpc::proto::{
    AgentEvent, AgentHello, CommandKind, Heartbeat, MachineRef, MachineReport, MachineRequest,
    RunningMachine,
};
use pistack_rpc::CommandQueue;
use pistack_store::{Store, StoreError};
use pistack_types::{ApiError, ApiResult, Id, Phase, Resources};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::driver::{DriverRegistry, HostDriver, MachineRuntime};
use crate::types::{new_host, Host, Machine, MachineKind, PowerState, Size};

// ---- RpcDriver ---------------------------------------------------------------

/// A host reached through the command queue. One per node; the registrar
/// creates them as agents say hello.
pub struct RpcDriver {
    queue: CommandQueue,
    node: String,
    /// Used to resolve `spec.size` into `Resources` for the request. Without
    /// it the request carries zero resources, which the mock does not mind.
    store: Option<Store>,
}

impl RpcDriver {
    pub fn for_node(queue: CommandQueue, node: impl Into<String>) -> Self {
        Self {
            queue,
            node: node.into(),
            store: None,
        }
    }

    /// Resolve sizes through `store` so the agent gets real resources.
    pub fn with_store(mut self, store: Store) -> Self {
        self.store = Some(store);
        self
    }

    pub fn node(&self) -> &str {
        &self.node
    }

    fn resources_for(&self, m: &Machine) -> Resources {
        let Some(store) = &self.store else {
            return Resources::default();
        };
        store
            .collection::<Size>()
            .resolve(None, &m.spec.size)
            .map(|s| s.spec.resources)
            .unwrap_or_default()
    }

    /// Build the wire request for `m`.
    pub fn request_for(&self, m: &Machine) -> MachineRequest {
        MachineRequest {
            id: m.meta.id.clone(),
            name: m.meta.name.clone(),
            kind: kind_to_wire(m.spec.kind),
            image: m.spec.image.clone(),
            resources: self.resources_for(m),
            networks: m.spec.networks.clone(),
            volumes: m.spec.volumes.clone(),
            user_data: m.spec.user_data.clone(),
            ssh_keys: m.spec.ssh_keys.clone(),
        }
    }

    async fn call<T: serde::Serialize>(
        &self,
        kind: CommandKind,
        payload: &T,
    ) -> ApiResult<serde_json::Value> {
        let payload =
            serde_json::to_value(payload).map_err(|e| ApiError::internal(e.to_string()))?;
        let ticket = self.queue.enqueue(&self.node, kind, payload)?;
        // Timeouts, queue-full and node-removed all map to `unavailable`
        // through `From<RpcError>`; the reconciler retries next pass.
        let result = ticket.wait().await?;
        result.into_api_result()
    }
}

#[async_trait]
impl HostDriver for RpcDriver {
    async fn start(&self, m: &Machine) -> ApiResult<MachineRuntime> {
        let req = self.request_for(m);
        let v = self.call(CommandKind::StartMachine, &req).await?;
        let rt: RunningMachine = serde_json::from_value(v).map_err(|e| {
            ApiError::internal(format!(
                "host {} returned a bad start result: {e}",
                self.node
            ))
        })?;
        Ok(runtime_from_wire(rt))
    }

    async fn stop(&self, id: &Id) -> ApiResult<()> {
        self.call(CommandKind::StopMachine, &MachineRef { id: id.clone() })
            .await
            .map(|_| ())
    }

    async fn delete(&self, id: &Id) -> ApiResult<()> {
        self.call(CommandKind::DeleteMachine, &MachineRef { id: id.clone() })
            .await
            .map(|_| ())
    }

    /// Always `Ok(empty)`: there is no list command on the wire. The agent
    /// reports what it runs in every heartbeat, and the [`Registrar`] folds
    /// those reports into machine status instead.
    async fn list(&self) -> ApiResult<Vec<MachineRuntime>> {
        Ok(Vec::new())
    }
}

pub fn kind_to_wire(k: MachineKind) -> pistack_rpc::proto::MachineKind {
    match k {
        MachineKind::Vm => pistack_rpc::proto::MachineKind::Vm,
        MachineKind::Container => pistack_rpc::proto::MachineKind::Container,
        MachineKind::Metal => pistack_rpc::proto::MachineKind::Metal,
    }
}

pub fn power_from_wire(p: pistack_rpc::proto::PowerState) -> PowerState {
    match p {
        pistack_rpc::proto::PowerState::On => PowerState::On,
        pistack_rpc::proto::PowerState::Off => PowerState::Off,
        pistack_rpc::proto::PowerState::Unknown => PowerState::Unknown,
    }
}

pub fn runtime_from_wire(rt: RunningMachine) -> MachineRuntime {
    MachineRuntime {
        id: rt.id,
        addresses: rt.addresses,
        power: power_from_wire(rt.power),
    }
}

// ---- Registrar ---------------------------------------------------------------

/// Label set on hosts the registrar creates, so they can be told apart from
/// the dev `local` host.
pub const AGENT_LABEL: &str = "pistack.io/agent";

/// Turns agent events into `Host` resources and registered drivers.
pub struct Registrar {
    store: Store,
    drivers: Arc<DriverRegistry>,
    queue: CommandQueue,
}

impl Registrar {
    pub fn new(store: Store, drivers: Arc<DriverRegistry>, queue: CommandQueue) -> Self {
        Self {
            store,
            drivers,
            queue,
        }
    }

    /// Consume agent events until `shutdown`.
    pub async fn run(self, shutdown: CancellationToken) {
        let mut rx = self.queue.subscribe_agent_events();
        loop {
            let ev = tokio::select! {
                _ = shutdown.cancelled() => break,
                ev = rx.recv() => ev,
            };
            match ev {
                Ok(ev) => {
                    let node = ev.node_name().to_owned();
                    if let Err(e) = self.handle(ev) {
                        tracing::warn!(node, error = %e, "forge: agent event not applied");
                    }
                }
                Err(RecvError::Lagged(n)) => {
                    tracing::debug!(skipped = n, "forge: agent events lagged; resyncing");
                    self.resync();
                }
                Err(RecvError::Closed) => break,
            }
        }
        tracing::info!("forge: registrar stopped");
    }

    /// Apply one event. Public so tests can drive it without the loop.
    pub fn handle(&self, ev: AgentEvent) -> anyhow::Result<()> {
        match ev {
            AgentEvent::Hello(h) => self.on_hello(&h).map(|_| ()),
            AgentEvent::Heartbeat(hb) => self.on_heartbeat(&hb),
        }
    }

    /// After a lag, treat every node the queue knows as freshly announced.
    fn resync(&self) {
        for n in self.queue.nodes() {
            let hello = AgentHello {
                node_name: n.name,
                capabilities: n.capabilities,
                resources: n.resources_total,
                version: n.version,
                labels: n.labels,
            };
            if let Err(e) = self.on_hello(&hello) {
                tracing::warn!(node = %hello.node_name, error = %e, "forge: resync failed");
            }
        }
    }

    fn host_for_node(&self, node: &str) -> anyhow::Result<Option<Host>> {
        let col = self.store.collection::<Host>();
        Ok(col
            .list(None)?
            .into_iter()
            .find(|h| h.spec.node_name == node))
    }

    fn register(&self, host: &Host) {
        if self.drivers.is_local(&host.meta.id) {
            // The dev mock host is served in-process; leave it alone.
            return;
        }
        let driver = RpcDriver::for_node(self.queue.clone(), host.spec.node_name.clone())
            .with_store(self.store.clone());
        self.drivers
            .register(host.meta.id.clone(), Arc::new(driver));
    }

    /// Create or refresh the host for a hello and register its driver.
    pub fn on_hello(&self, h: &AgentHello) -> anyhow::Result<Host> {
        let col = self.store.collection::<Host>();
        let now = Utc::now();
        let host = match self.host_for_node(&h.node_name)? {
            Some(existing) => col.modify(&existing.meta.id, |x| {
                x.spec.capacity = h.resources;
                x.spec.drivers = h.capabilities.clone();
                for (k, v) in &h.labels {
                    x.meta.labels.insert(k.clone(), v.clone());
                }
                x.meta.labels.insert(AGENT_LABEL.into(), "true".into());
                if !h.version.is_empty() {
                    x.meta
                        .annotations
                        .insert("pistack.io/agent-version".into(), h.version.clone());
                }
                x.status.phase = Phase::Ready;
                x.status.message.clear();
                x.status.last_seen = Some(now);
                Ok(())
            })?,
            None => {
                let mut labels: BTreeMap<String, String> = h.labels.clone();
                labels.insert(AGENT_LABEL.into(), "true".into());
                let mut host = new_host(
                    &h.node_name,
                    &h.node_name,
                    h.resources,
                    h.capabilities.clone(),
                    labels,
                );
                if !h.version.is_empty() {
                    host.meta
                        .annotations
                        .insert("pistack.io/agent-version".into(), h.version.clone());
                }
                match col.create(host) {
                    Ok(h) => h,
                    // Raced with another hello for the same node.
                    Err(StoreError::Conflict(_)) => self
                        .host_for_node(&h.node_name)?
                        .ok_or_else(|| anyhow::anyhow!("host {} vanished", h.node_name))?,
                    Err(e) => return Err(e.into()),
                }
            }
        };
        self.register(&host);
        tracing::info!(
            host = %host.meta.id,
            node = %h.node_name,
            drivers = ?h.capabilities,
            capacity = ?h.resources,
            "forge: host registered from agent hello"
        );
        Ok(host)
    }

    /// Refresh `last_seen` and fold machine reports into status.
    pub fn on_heartbeat(&self, hb: &Heartbeat) -> anyhow::Result<()> {
        let Some(host) = self.host_for_node(&hb.node_name)? else {
            // No hello yet (control plane restarted): the ack already told
            // the agent `known = false`, so a hello is on its way.
            tracing::debug!(node = %hb.node_name, "forge: heartbeat from unregistered node");
            return Ok(());
        };
        if self.drivers.get(&host.meta.id).is_none() {
            self.register(&host);
        }
        let col = self.store.collection::<Host>();
        let now = Utc::now();
        match col.modify(&host.meta.id, |x| {
            x.status.last_seen = Some(now);
            if x.status.phase == Phase::Error && x.status.message.starts_with("no report") {
                x.status.phase = Phase::Ready;
                x.status.message.clear();
            }
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        // `status.free` stays with the reconciler, which recomputes it from
        // placements every pass; the agent's figure is informational.
        tracing::trace!(node = %hb.node_name, free = ?hb.resources_free, "forge: heartbeat");
        for r in &hb.machines {
            if let Err(e) = self.apply_report(&host.meta.id, r) {
                tracing::debug!(machine = %r.id, error = %e, "forge: report not applied");
            }
        }
        Ok(())
    }

    /// Update a `Ready` machine's observed addresses/power from the host's
    /// report when they differ. Machines mid-transition are left to the
    /// reconciler, which owns those phases.
    fn apply_report(&self, host: &Id, r: &MachineReport) -> anyhow::Result<()> {
        let col = self.store.collection::<Machine>();
        let Some(m) = col.try_get(&r.id)? else {
            return Ok(());
        };
        if m.status.host.as_ref() != Some(host) || m.status.phase != Phase::Ready {
            return Ok(());
        }
        let power = r.power.map(power_from_wire);
        let addrs_differ = !r.addresses.is_empty() && r.addresses != m.status.addresses;
        let power_differs = power.is_some_and(|p| p != PowerState::Unknown && p != m.status.power);
        if !addrs_differ && !power_differs {
            return Ok(());
        }
        match col.modify(&m.meta.id, |x| {
            if addrs_differ {
                x.status.addresses = r.addresses.clone();
            }
            if let (true, Some(p)) = (power_differs, power) {
                x.status.power = p;
            }
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
            Err(e) => Err(e.into()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{new_machine, MachineSpec};
    use pistack_rpc::proto::CommandResult;
    use pistack_rpc::QueueConfig;
    use std::time::Duration;

    fn machine(name: &str) -> Machine {
        new_machine(
            name,
            None,
            MachineSpec {
                size: "s.1x1".into(),
                image: "ubuntu".into(),
                ..Default::default()
            },
        )
    }

    fn hello(node: &str) -> AgentHello {
        AgentHello {
            node_name: node.into(),
            capabilities: vec!["kvm".into(), "mock".into()],
            resources: Resources {
                vcpus: 8,
                memory_mib: 16384,
                disk_gib: 200,
                gpus: 0,
            },
            version: "t".into(),
            labels: [("rack".to_string(), "r1".to_string())]
                .into_iter()
                .collect(),
        }
    }

    #[tokio::test]
    async fn rpc_driver_round_trips_through_queue() {
        let queue = CommandQueue::default();
        let driver = RpcDriver::for_node(queue.clone(), "h1");
        let m = machine("web");

        // Simulate the agent: pull the command, answer it.
        let q = queue.clone();
        let mid = m.meta.id.clone();
        let agent = tokio::spawn(async move {
            let cmds = q.next("h1", Duration::from_secs(5), 16).await;
            assert_eq!(cmds.len(), 1);
            assert_eq!(cmds[0].kind, CommandKind::StartMachine);
            let req: MachineRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
            assert_eq!(req.id, mid);
            assert_eq!(req.image, "ubuntu");
            let rt = RunningMachine {
                id: req.id,
                addresses: vec!["10.1.0.9".into()],
                power: pistack_rpc::proto::PowerState::On,
            };
            q.ack(
                "h1",
                CommandResult::ok(cmds[0].id).with_payload(serde_json::to_value(rt).unwrap()),
            )
            .unwrap();

            let cmds = q.next("h1", Duration::from_secs(5), 16).await;
            assert_eq!(cmds[0].kind, CommandKind::StopMachine);
            let r: MachineRef = serde_json::from_value(cmds[0].payload.clone()).unwrap();
            q.ack(
                "h1",
                CommandResult::from_api_error(
                    cmds[0].id,
                    &ApiError::not_found(format!("{} not here", r.id)),
                ),
            )
            .unwrap();
        });

        let rt = driver.start(&m).await.unwrap();
        assert_eq!(rt.addresses, vec!["10.1.0.9"]);
        assert_eq!(rt.power, PowerState::On);
        let e = driver.stop(&m.meta.id).await.unwrap_err();
        assert_eq!(e.code, pistack_types::error::ErrorCode::NotFound);
        assert!(driver.list().await.unwrap().is_empty());
        agent.await.unwrap();
    }

    #[tokio::test]
    async fn rpc_driver_timeout_is_unavailable() {
        let queue = CommandQueue::new(QueueConfig {
            command_timeout: Duration::from_millis(50),
            capacity: 4,
            heartbeat_interval: Duration::from_secs(5),
        });
        let driver = RpcDriver::for_node(queue, "offline");
        let e = driver
            .delete(&Id::new(pistack_types::kind::MACHINE))
            .await
            .unwrap_err();
        assert_eq!(e.code, pistack_types::error::ErrorCode::Unavailable);
    }

    #[tokio::test]
    async fn registrar_creates_host_and_applies_reports() {
        let store = Store::in_memory().unwrap();
        let drivers = Arc::new(DriverRegistry::new());
        let queue = CommandQueue::default();
        let reg = Registrar::new(store.clone(), drivers.clone(), queue.clone());

        let host = reg.on_hello(&hello("h1")).unwrap();
        assert_eq!(host.spec.node_name, "h1");
        assert_eq!(host.spec.drivers, vec!["kvm", "mock"]);
        assert_eq!(host.meta.labels["rack"], "r1");
        assert_eq!(host.status.phase, Phase::Ready);
        assert!(drivers.get(&host.meta.id).is_some());
        assert!(!drivers.is_local(&host.meta.id));

        // Second hello updates in place rather than duplicating.
        let mut h2 = hello("h1");
        h2.resources.vcpus = 16;
        let again = reg.on_hello(&h2).unwrap();
        assert_eq!(again.meta.id, host.meta.id);
        assert_eq!(again.spec.capacity.vcpus, 16);
        assert_eq!(store.collection::<Host>().list(None).unwrap().len(), 1);

        // A Ready machine on this host picks up the reported address/power.
        let mut m = machine("web");
        m.status.host = Some(host.meta.id.clone());
        m.status.phase = Phase::Ready;
        m.status.power = PowerState::On;
        m.status.addresses = vec!["10.0.0.1".into()];
        let m = store.collection::<Machine>().create(m).unwrap();
        reg.on_heartbeat(&Heartbeat {
            node_name: "h1".into(),
            resources_free: Resources::default(),
            machines: vec![MachineReport {
                id: m.meta.id.clone(),
                phase: Phase::Ready,
                message: String::new(),
                resources: None,
                addresses: vec!["10.0.0.7".into()],
                power: Some(pistack_rpc::proto::PowerState::Off),
            }],
            at: Utc::now(),
        })
        .unwrap();
        let m = store.collection::<Machine>().get(&m.meta.id).unwrap();
        assert_eq!(m.status.addresses, vec!["10.0.0.7"]);
        assert_eq!(m.status.power, PowerState::Off);
        let h = store.collection::<Host>().get(&host.meta.id).unwrap();
        assert!(h.status.last_seen.unwrap() >= host.status.last_seen.unwrap());

        // Heartbeat from an unknown node is ignored, not an error.
        reg.on_heartbeat(&Heartbeat {
            node_name: "ghost".into(),
            resources_free: Resources::default(),
            machines: vec![],
            at: Utc::now(),
        })
        .unwrap();
        assert_eq!(store.collection::<Host>().list(None).unwrap().len(), 1);
    }
}
