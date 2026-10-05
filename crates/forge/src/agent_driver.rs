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
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use pistack_rpc::proto::{
    AgentEvent, AgentHello, CommandKind, Heartbeat, MachineRef, MachineReport, MachineRequest,
    MigrateRequest, RunningMachine,
};
use pistack_rpc::CommandQueue;
use pistack_store::{Store, StoreError};
use pistack_types::{ApiError, ApiResult, Id, Phase, Resources};
use tokio::sync::broadcast::error::RecvError;
use tokio_util::sync::CancellationToken;

use crate::driver::{DriverRegistry, HostDriver, MachineRuntime};
use crate::types::{new_host, usable_underlay, Host, Machine, MachineKind, PowerState, Size};

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
    ///
    /// A bare image name is replaced when the store has a ready `img` of
    /// that name whose `file_path` or `backing_ref` is an absolute file
    /// that exists. Anything else, including `ubuntu-24.04` and a
    /// `sha256:` blob id, is sent unchanged so anvil can still resolve
    /// `bases/<name>.qcow2`.
    pub fn request_for(&self, m: &Machine) -> MachineRequest {
        MachineRequest {
            id: m.meta.id.clone(),
            name: m.meta.name.clone(),
            kind: kind_to_wire(m.spec.kind),
            image: resolve_boot_image(self.store.as_ref(), &m.spec.image),
            resources: self.resources_for(m),
            networks: m.spec.networks.clone(),
            nics: self
                .store
                .as_ref()
                .and_then(|store| crate::nic::attachments(store, m).ok())
                .unwrap_or_default(),
            volumes: m.spec.volumes.clone(),
            user_data: m.spec.user_data.clone(),
            ssh_keys: m.spec.ssh_keys.clone(),
            overlay: None,
            bmc_address: m.spec.bmc_address.clone(),
            bmc_user: m.spec.bmc_user.clone(),
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
        let mut req = self.request_for(m);
        if crate::overlay::enabled() && crate::nic::wants_port(m) {
            let store = self
                .store
                .as_ref()
                .ok_or_else(|| ApiError::internal("overlay placement needs the store"))?;
            req.nics = crate::nic::attachments(store, m)?;
            let plans = crate::overlay::plans_for_machine(store, m)?;
            for (node, plan) in &plans {
                if node == &self.node {
                    req.overlay = Some(plan.clone());
                } else if let Err(e) = self.call(CommandKind::SyncOverlay, plan).await {
                    tracing::warn!(
                        node,
                        error = %e,
                        "forge: overlay peer was not updated; the next pass retries"
                    );
                }
            }
        }
        if crate::nic::wants_port(m) && req.nics.is_empty() {
            return Err(ApiError::unavailable(format!(
                "machine {} has no address on its network",
                m.meta.id
            )));
        }
        if req.overlay.is_none() && req.nics.iter().any(|n| n.segment_id.is_some()) {
            return Err(ApiError::unavailable(format!(
                "machine {} is not on a host that joined its network segment",
                m.meta.id
            )));
        }
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

/// Catalog row, read as JSON so forge does not depend on mold.
#[derive(serde::Deserialize)]
struct CatalogImage {
    name: String,
    #[serde(default)]
    status: CatalogStatus,
}

#[derive(serde::Deserialize, Default)]
struct CatalogStatus {
    #[serde(default)]
    phase: String,
    #[serde(default)]
    backing_ref: Option<String>,
    #[serde(default)]
    file_path: Option<String>,
}

fn resolve_boot_image(store: Option<&Store>, image: &str) -> String {
    if Path::new(image).is_absolute() {
        return image.to_owned();
    }
    store
        .and_then(|store| catalog_backing(store, image))
        .unwrap_or_else(|| image.to_owned())
}

fn catalog_backing(store: &Store, name: &str) -> Option<String> {
    let rows = store.raw_scan("resources", "img/").ok()?;
    for (_, bytes) in rows {
        let Ok(view) = serde_json::from_slice::<CatalogImage>(&bytes) else {
            continue;
        };
        if view.name != name || view.status.phase != "ready" {
            continue;
        }
        let Some(path) = view
            .status
            .file_path
            .filter(|p| !p.is_empty())
            .or(view.status.backing_ref)
        else {
            continue;
        };
        let file = Path::new(&path);
        if file.is_absolute() && file.is_file() {
            return Some(path);
        }
    }
    None
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

/// `qemu+ssh://<underlay>/system` for a live-migration destination.
/// A missing or non-private underlay is an error. There is no path that
/// migrates without one, even when the machine names no network.
pub fn migrate_dest(host: &Host) -> ApiResult<String> {
    let ip = usable_underlay(host.spec.underlay_ip.as_deref()).ok_or_else(|| {
        ApiError::unprocessable(format!(
            "host {} has no usable private underlay",
            host.meta.name
        ))
    })?;
    Ok(format!("qemu+ssh://{ip}/system"))
}

/// Queue a live migration of `machine` onto `dest`.
///
/// The command goes to the source node's agent. A refusal leaves the
/// placement alone. Phase stays `Updating` so the reconciler does not start
/// the guest again. `status.host` changes when the destination reports the
/// machine.
pub fn begin_migration(
    store: &Store,
    queue: &CommandQueue,
    machine: &Machine,
    dest: &Host,
) -> ApiResult<Machine> {
    let source_id =
        machine.status.host.clone().ok_or_else(|| {
            ApiError::conflict(format!("machine {} has no host", machine.meta.id))
        })?;
    if source_id == dest.meta.id {
        return Err(ApiError::conflict(
            "destination is the machine's current host",
        ));
    }
    if dest.status.phase != Phase::Ready {
        return Err(ApiError::conflict(format!(
            "host {} is not ready",
            dest.meta.name
        )));
    }
    // Underlay is required whether or not `spec.networks` names one.
    let dest_uri = migrate_dest(dest)?;
    let source = store.collection::<Host>().get(&source_id)?;
    if source.spec.node_name.is_empty() {
        return Err(ApiError::unprocessable(format!(
            "host {} has no node name",
            source.meta.name
        )));
    }
    let payload = MigrateRequest {
        id: machine.meta.id.clone(),
        dest: dest_uri,
    };
    let body = serde_json::to_value(&payload).map_err(|e| ApiError::internal(e.to_string()))?;
    let ticket = queue.enqueue(&source.spec.node_name, CommandKind::MigrateMachine, body)?;
    let updated = store
        .collection::<Machine>()
        .modify(&machine.meta.id, |m| {
            m.status.phase = Phase::Updating;
            m.status.message = format!("migrating {}", dest.meta.id);
            Ok(())
        })
        .map_err(ApiError::from)?;
    // The HTTP handler must return now. A failed or timed-out command
    // leaves `Error` on the source host. Success stays `Updating` until
    // the destination reports the machine.
    let store = store.clone();
    let machine_id = machine.meta.id.clone();
    let expect = format!("migrating {}", dest.meta.id);
    tokio::spawn(async move {
        let failed = match ticket.wait().await {
            Ok(result) if result.ok => None,
            Ok(result) => Some(if result.message.is_empty() {
                "migration failed".to_owned()
            } else {
                result.message
            }),
            Err(e) => Some(e.to_string()),
        };
        let Some(message) = failed else {
            return;
        };
        let _ = store.collection::<Machine>().modify(&machine_id, |m| {
            if m.status.phase == Phase::Updating && m.status.message == expect {
                m.status.phase = Phase::Error;
                m.status.message = message.clone();
            }
            Ok(())
        });
    });
    Ok(updated)
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
                underlay_ip: n.underlay_ip,
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
        let underlay = usable_underlay(h.underlay_ip.as_deref());
        let host = match self.host_for_node(&h.node_name)? {
            Some(existing) => col.modify(&existing.meta.id, |x| {
                x.spec.capacity = h.resources;
                x.spec.drivers = h.capabilities.clone();
                x.spec.underlay_ip = underlay.clone();
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
                host.spec.underlay_ip = underlay;
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
        // Capacity is the host total from hello, not `resources_free`.
        gauge::record_host(
            gauge::samples().as_ref(),
            &host.spec.node_name,
            host.spec.capacity.memory_mib,
            host.spec.capacity.vcpus,
        );
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
        if m.status.phase == Phase::Updating {
            let dest = m.status.message.strip_prefix("migrating ").unwrap_or("");
            if dest == host.to_string() {
                let power = r.power.map(power_from_wire);
                match col.modify(&m.meta.id, |x| {
                    x.status.host = Some(host.clone());
                    x.status.phase = Phase::Ready;
                    x.status.message.clear();
                    if !r.addresses.is_empty() {
                        x.status.addresses = r.addresses.clone();
                    }
                    if let Some(p) = power {
                        if p != PowerState::Unknown {
                            x.status.power = p;
                        }
                    }
                    Ok(())
                }) {
                    Ok(_) | Err(StoreError::NotFound(_)) => Ok(()),
                    Err(e) => Err(e.into()),
                }
            } else {
                Ok(())
            }
        } else if m.status.host.as_ref() != Some(host) || m.status.phase != Phase::Ready {
            return Ok(());
        } else {
            let power = r.power.map(power_from_wire);
            let addrs_differ = !r.addresses.is_empty() && r.addresses != m.status.addresses;
            let power_differs =
                power.is_some_and(|p| p != PowerState::Unknown && p != m.status.power);
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::{new_machine, MachineSpec};
    use pistack_rpc::proto::CommandResult;
    use pistack_rpc::QueueConfig;
    use pistack_types::{Kind, Meta};
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
            underlay_ip: None,
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
        let text = gauge::Gauge.client(store.clone()).metrics_text();
        assert!(
            text.lines().any(|l| {
                l.contains("pistack_host_memory_mib")
                    && l.contains("node=\"h1\"")
                    && (l.contains(" 16384") || l.contains(" 16384.0"))
            }),
            "{text}"
        );
        assert!(
            text.lines().any(|l| {
                l.contains("pistack_host_vcpus")
                    && l.contains("node=\"h1\"")
                    && (l.ends_with(" 16") || l.ends_with(" 16.0"))
            }),
            "{text}"
        );

        // Heartbeat from an unknown node is ignored, not an error.
        reg.on_heartbeat(&Heartbeat {
            node_name: "ghost".into(),
            resources_free: Resources::default(),
            machines: vec![],
            at: Utc::now(),
        })
        .unwrap();
        assert_eq!(store.collection::<Host>().list(None).unwrap().len(), 1);
        let text = gauge::Gauge.client(store).metrics_text();
        assert!(
            !text.lines().any(|l| l.contains("node=\"ghost\"")),
            "{text}"
        );
    }

    #[tokio::test]
    async fn on_hello_stores_and_replaces_underlay() {
        let store = Store::in_memory().unwrap();
        let drivers = Arc::new(DriverRegistry::new());
        let queue = CommandQueue::default();
        let reg = Registrar::new(store.clone(), drivers, queue.clone());

        let mut h = hello("h1");
        h.underlay_ip = Some("10.1.0.2".into());
        let host = reg.on_hello(&h).unwrap();
        assert_eq!(host.spec.underlay_ip.as_deref(), Some("10.1.0.2"));

        h.underlay_ip = Some("100.64.8.8".into());
        let again = reg.on_hello(&h).unwrap();
        assert_eq!(again.meta.id, host.meta.id);
        assert_eq!(again.spec.underlay_ip.as_deref(), Some("100.64.8.8"));

        h.underlay_ip = None;
        let cleared = reg.on_hello(&h).unwrap();
        assert_eq!(cleared.spec.underlay_ip, None);

        h.underlay_ip = Some("8.8.8.8".into());
        let rejected = reg.on_hello(&h).unwrap();
        assert_eq!(rejected.spec.underlay_ip, None);

        h.underlay_ip = Some("192.168.5.9".into());
        queue.hello(&h);
        reg.resync();
        let kept = store.collection::<Host>().get(&host.meta.id).unwrap();
        assert_eq!(kept.spec.underlay_ip.as_deref(), Some("192.168.5.9"));
    }

    #[derive(Clone, serde::Serialize, serde::Deserialize)]
    struct ImgRow {
        #[serde(flatten)]
        meta: Meta,
        #[serde(default)]
        spec: serde_json::Value,
        #[serde(default)]
        status: serde_json::Value,
    }

    impl Kind for ImgRow {
        const KIND: &'static str = "img";
        fn meta(&self) -> &Meta {
            &self.meta
        }
        fn meta_mut(&mut self) -> &mut Meta {
            &mut self.meta
        }
    }

    fn put_image(store: &Store, name: &str, phase: &str, backing: Option<&str>) {
        put_image_at(store, name, phase, backing, None);
    }

    fn put_image_at(
        store: &Store,
        name: &str,
        phase: &str,
        backing: Option<&str>,
        file_path: Option<&str>,
    ) {
        let meta = Meta::new(pistack_types::kind::IMAGE, name, None);
        let mut status = serde_json::Map::new();
        status.insert("phase".into(), serde_json::Value::String(phase.into()));
        if let Some(b) = backing {
            status.insert("backing_ref".into(), serde_json::Value::String(b.into()));
        }
        if let Some(p) = file_path {
            status.insert("file_path".into(), serde_json::Value::String(p.into()));
        }
        store
            .collection::<ImgRow>()
            .create(ImgRow {
                meta,
                spec: serde_json::json!({}),
                status: serde_json::Value::Object(status),
            })
            .unwrap();
    }

    #[test]
    fn catalog_ready_file_replaces_image_name() {
        let store = Store::in_memory().unwrap();
        let path = std::env::temp_dir().join(format!(
            "pistack-boot-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        std::fs::write(&path, b"disk").unwrap();
        let path_str = path.to_string_lossy().into_owned();
        put_image(&store, "custom-base", "ready", Some(&path_str));
        // Not ready, and a sha256 ref, must not win over the file above.
        put_image(&store, "custom-base-pending", "pending", Some(&path_str));
        put_image(
            &store,
            "hashed",
            "ready",
            Some("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
        );
        put_image_at(
            &store,
            "from-catalog",
            "ready",
            Some("sha256:0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef"),
            Some(&path_str),
        );

        let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
        let mut m = machine("web");
        m.spec.image = "custom-base".into();
        assert_eq!(driver.request_for(&m).image, path_str);
        m.spec.image = "from-catalog".into();
        assert_eq!(driver.request_for(&m).image, path_str);

        m.spec.image = path.display().to_string();
        assert_eq!(driver.request_for(&m).image, path.display().to_string());
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_catalog_image_keeps_the_name() {
        let store = Store::in_memory().unwrap();
        let absent = std::env::temp_dir().join(format!(
            "pistack-boot-missing-{}-{}",
            std::process::id(),
            uuid::Uuid::now_v7()
        ));
        let absent_str = absent.to_string_lossy().into_owned();
        put_image(&store, "gone", "ready", Some(&absent_str));
        put_image(&store, "ubuntu-24.04", "error", None);

        let driver = RpcDriver::for_node(CommandQueue::default(), "h1").with_store(store);
        let mut m = machine("web");
        m.spec.image = "ubuntu-24.04".into();
        assert_eq!(driver.request_for(&m).image, "ubuntu-24.04");
        m.spec.image = "no-such-image".into();
        assert_eq!(driver.request_for(&m).image, "no-such-image");
        m.spec.image = "gone".into();
        assert_eq!(driver.request_for(&m).image, "gone");

        let bare = RpcDriver::for_node(CommandQueue::default(), "h1");
        m.spec.image = "ubuntu-24.04".into();
        assert_eq!(bare.request_for(&m).image, "ubuntu-24.04");
    }

    fn placed(name: &str, host: &Id, networks: &[&str]) -> Machine {
        let mut m = machine(name);
        m.status.host = Some(host.clone());
        m.status.phase = Phase::Ready;
        m.spec.networks = networks.iter().map(|s| (*s).to_string()).collect();
        m
    }

    fn host_with(name: &str, node: &str, underlay: Option<&str>, phase: Phase) -> Host {
        let mut h = new_host(
            name,
            node,
            Resources {
                vcpus: 4,
                memory_mib: 8192,
                disk_gib: 100,
                gpus: 0,
            },
            vec!["kvm".into()],
            std::collections::BTreeMap::new(),
        );
        h.spec.underlay_ip = underlay.map(str::to_owned);
        h.status.phase = phase;
        h
    }

    #[tokio::test]
    async fn migrate_refuses_a_host_without_underlay_and_enqueues_when_present() {
        use pistack_rpc::proto::MigrateRequest;

        let store = Store::in_memory().unwrap();
        let queue = CommandQueue::default();
        let hosts = store.collection::<Host>();
        let src = hosts
            .create(host_with("src", "node-src", None, Phase::Ready))
            .unwrap();
        let bare = hosts
            .create(host_with("bare", "node-bare", None, Phase::Ready))
            .unwrap();
        let down = hosts
            .create(host_with(
                "down",
                "node-down",
                Some("10.9.0.1"),
                Phase::Error,
            ))
            .unwrap();
        let dest = hosts
            .create(host_with(
                "dest",
                "node-dest",
                Some("10.1.0.2"),
                Phase::Ready,
            ))
            .unwrap();
        assert!(migrate_dest(&bare).is_err());
        assert_eq!(migrate_dest(&dest).unwrap(), "qemu+ssh://10.1.0.2/system");

        let machines = store.collection::<Machine>();
        let named = machines
            .create(placed("web", &src.meta.id, &["tenant"]))
            .unwrap();
        let quiet = machines.create(placed("quiet", &src.meta.id, &[])).unwrap();

        let err = begin_migration(&store, &queue, &named, &bare).unwrap_err();
        assert!(err.message.contains("underlay"));
        let still = machines.get(&named.meta.id).unwrap();
        assert_eq!(still.status.host.as_ref(), Some(&src.meta.id));
        assert_eq!(queue.pending_count(), 0);

        let err = begin_migration(&store, &queue, &quiet, &bare).unwrap_err();
        assert!(err.message.contains("underlay"));
        assert_eq!(
            machines.get(&quiet.meta.id).unwrap().status.host.as_ref(),
            Some(&src.meta.id)
        );

        let err = begin_migration(&store, &queue, &named, &src).unwrap_err();
        assert!(err.message.contains("current host"));
        let err = begin_migration(&store, &queue, &named, &down).unwrap_err();
        assert!(err.message.contains("not ready"));
        let mut loose = machine("loose");
        loose.status.phase = Phase::Ready;
        let loose = machines.create(loose).unwrap();
        assert!(begin_migration(&store, &queue, &loose, &dest).is_err());
        assert_eq!(queue.pending_count(), 0);
        assert_eq!(
            machines.get(&named.meta.id).unwrap().status.host.as_ref(),
            Some(&src.meta.id)
        );

        let moved = begin_migration(&store, &queue, &named, &dest).unwrap();
        assert_eq!(moved.status.host.as_ref(), Some(&src.meta.id));
        assert_eq!(moved.status.phase, Phase::Updating);
        assert_eq!(moved.status.message, format!("migrating {}", dest.meta.id));
        let cmds = queue.next("node-src", Duration::from_secs(1), 8).await;
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].kind, CommandKind::MigrateMachine);
        assert_eq!(cmds[0].kind.to_string(), "migrate_machine");
        let req: MigrateRequest = serde_json::from_value(cmds[0].payload.clone()).unwrap();
        assert_eq!(req.id, named.meta.id);
        assert_eq!(req.dest, "qemu+ssh://10.1.0.2/system");

        queue
            .ack(
                "node-src",
                CommandResult::failed(cmds[0].id, "virsh migrate failed"),
            )
            .unwrap();
        let mut failed = None;
        for _ in 0..50 {
            let cur = machines.get(&named.meta.id).unwrap();
            if cur.status.phase == Phase::Error {
                failed = Some(cur);
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let failed = failed.expect("a failed migrate leaves Error");
        assert_eq!(failed.status.host.as_ref(), Some(&src.meta.id));
        assert!(failed.status.message.contains("virsh migrate failed"));
    }
}
