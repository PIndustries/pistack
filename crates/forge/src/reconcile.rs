// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The machine reconciler: makes `status` true to `spec`.
//!
//! Wakes on every `mach` store event and every [`TICK`], then runs one
//! idempotent pass over all hosts and machines:
//!
//! * hosts: locally-served hosts get a heartbeat; hosts unseen for
//!   [`HOST_STALE_AFTER`] go to `Error`; `status.free` is recomputed from
//!   the machines placed on each host.
//! * machines: `Pending` -> scheduler -> `Creating` -> driver start ->
//!   `Ready`; delete requests -> `Deleting` -> driver stop/delete -> gone;
//!   desired power and reboot counters are applied to `Ready` machines;
//!   machines on an unreachable host get a `HostReachable=false` condition.
//!   A machine annotated [`WAIT_VOLUME_ANNOTATION`] is left unplaced and
//!   unstarted. [`NODE_PIN_ANNOTATION`] limits placement to that host.

use crate::driver::{DriverRegistry, HostDriver};
use crate::scheduler::{self, Allocations, Strategy};
use crate::types::{
    delete_requested, DesiredPower, Host, Machine, MachineStatus, PowerState, Size,
    NODE_PIN_ANNOTATION, WAIT_VOLUME_ANNOTATION,
};
use chrono::Utc;
use pistack_rpc::proto::CommandKind;
use pistack_rpc::CommandQueue;
use pistack_store::{Collection, Store, StoreError};
use pistack_types::error::ErrorCode;
use pistack_types::{kind, ApiError, Id, ModuleContext, Phase, Resources};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// How often a pass runs even when nothing changed.
pub const TICK: Duration = Duration::from_secs(2);
/// A host that has not reported for this long is `Error`.
pub const HOST_STALE_AFTER: Duration = Duration::from_secs(30);
/// A host whose last overlay apply failed gets the same plan again after
/// this long, even when nothing in the plan changed.
pub const OVERLAY_RETRY: Duration = Duration::from_secs(30);

/// The overlay plan last sent to one host, and how that send went.
#[derive(Clone, Debug, PartialEq, Eq)]
struct OverlaySent {
    body: String,
    /// The host's hello time when it was sent. A new hello means the agent
    /// restarted, so the plan goes again.
    hello: Option<chrono::DateTime<Utc>>,
    /// Set when the agent answered with a failure or never answered.
    failed: Option<std::time::Instant>,
}

/// Whether `node` needs this plan sent (again).
fn overlay_due(
    prev: Option<&OverlaySent>,
    body: &str,
    hello: Option<chrono::DateTime<Utc>>,
) -> bool {
    match prev {
        None => true,
        Some(s) => {
            s.body != body
                || s.hello != hello
                || s.failed.is_some_and(|t| t.elapsed() >= OVERLAY_RETRY)
        }
    }
}

type OverlaySentMap = Arc<parking_lot::Mutex<BTreeMap<String, OverlaySent>>>;

/// Mark the send as failed, unless a newer plan was sent since.
fn overlay_failed(sent: &OverlaySentMap, node: &str, body: &str) {
    if let Some(s) = sent.lock().get_mut(node) {
        if s.body == body {
            s.failed = Some(std::time::Instant::now());
        }
    }
}

pub struct Reconciler {
    store: Store,
    drivers: Arc<DriverRegistry>,
    strategy: Strategy,
    queue: Option<CommandQueue>,
    /// Last overlay document sent to each node. A changed plan, a new agent
    /// hello, or a failed apply sends it again.
    overlay_sent: OverlaySentMap,
}

impl Reconciler {
    pub fn new(store: Store, drivers: Arc<DriverRegistry>, strategy: Strategy) -> Self {
        Self {
            store,
            drivers,
            strategy,
            queue: None,
            overlay_sent: Arc::new(parking_lot::Mutex::new(BTreeMap::new())),
        }
    }

    /// When set, a pass refreshes overlay plans on hosts that have joined.
    pub fn with_queue(mut self, queue: CommandQueue) -> Self {
        self.queue = Some(queue);
        self
    }

    /// Run only while this node is the cluster leader. Idle on followers;
    /// restart on promotion; stop on demotion or shutdown.
    pub async fn run(self, ctx: ModuleContext) {
        let this = std::sync::Arc::new(self);
        let leadership = ctx.leadership.clone();
        leadership
            .run_while_leader(ctx.shutdown.clone(), move |active| {
                let this = this.clone();
                async move { this.run_loop(active).await }
            })
            .await;
    }

    pub(crate) async fn run_loop(&self, active: pistack_types::CancellationToken) {
        // A new term starts from nothing: every host gets the current plan
        // once, so a failed apply remembered by an earlier term (here or on
        // another server) is not forgotten.
        self.overlay_sent.lock().clear();
        let mut rx = self.store.events().subscribe();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = active.cancelled() => break,
                _ = tick.tick() => {}
                ev = rx.recv() => match ev {
                    Ok(e) if e.kind == kind::MACHINE => {}
                    Ok(_) => continue,
                    Err(RecvError::Lagged(n)) => tracing::debug!(skipped = n, "forge: event bus lagged"),
                    Err(RecvError::Closed) => break,
                },
            }
            if let Err(e) = self.reconcile_once().await {
                if e.to_string() == "not the leader" {
                    tracing::debug!(error = %e, "forge: reconcile stopped; not the leader");
                } else {
                    tracing::warn!(error = %e, "forge: reconcile pass failed");
                }
            }
        }
        tracing::info!("forge: reconciler stopped");
    }

    fn hosts(&self) -> Collection<Host> {
        self.store.collection::<Host>()
    }
    fn machines(&self) -> Collection<Machine> {
        self.store.collection::<Machine>()
    }
    fn sizes(&self) -> Collection<Size> {
        self.store.collection::<Size>()
    }

    /// One full pass. Public so tests can drive it without the loop.
    pub async fn reconcile_once(&self) -> anyhow::Result<()> {
        let machines = self.machines().list(None)?;
        let sizes: BTreeMap<String, Size> = self
            .sizes()
            .list(None)?
            .into_iter()
            .map(|s| (s.meta.name.clone(), s))
            .collect();
        let size_of = |m: &Machine| -> Option<Resources> {
            sizes
                .get(&m.spec.size)
                .or_else(|| {
                    sizes
                        .values()
                        .find(|s| s.meta.id.to_string() == m.spec.size)
                })
                .map(|s| s.spec.resources)
        };

        let hosts = self.reconcile_hosts(&machines, &size_of)?;

        // Resources committed during this pass but not yet in host.status.free,
        // plus the names already on each host (for affinity hints). Existing
        // placements are already subtracted from status.free by
        // reconcile_hosts, so they contribute names only.
        let mut alloc = Allocations::new();
        for m in &machines {
            if let Some(h) = &m.status.host {
                alloc.add(h, &m.meta.name, Resources::default());
            }
        }

        for m in machines {
            let id = m.meta.id.clone();
            let res = if delete_requested(&m) {
                self.teardown(m).await
            } else {
                self.converge(m, &hosts, &mut alloc, &size_of).await
            };
            if let Err(e) = res {
                if e.to_string() == "not the leader" {
                    tracing::debug!(machine = %id, error = %e, "forge: machine reconcile stopped; not the leader");
                } else {
                    tracing::warn!(machine = %id, error = %e, "forge: machine reconcile failed");
                }
            }
        }
        // Binding record for fabric ports. Runs even when the overlay
        // backend is off. A machine with no host does not clear one.
        self.bind_port_hosts()?;
        self.sync_overlay().await;
        Ok(())
    }

    /// Set `status.host` on each fabric port plugged into a placed machine.
    /// A port that already names that host is not written again.
    fn bind_port_hosts(&self) -> anyhow::Result<()> {
        let machines = self.machines().list(None)?;
        let ports = self.store.collection::<fabric::Port>();
        let listed = ports.list(None)?;
        for machine in machines {
            let Some(host) = machine.status.host.as_ref() else {
                continue;
            };
            let recorded = host.to_string();
            for port in &listed {
                if port.spec.device.as_ref() != Some(&machine.meta.id) {
                    continue;
                }
                if port.status.host.as_deref() == Some(recorded.as_str()) {
                    continue;
                }
                let next = recorded.clone();
                match ports.modify(&port.meta.id, |p| {
                    p.status.host = Some(next.clone());
                    Ok(())
                }) {
                    Ok(_) => {}
                    Err(pistack_store::StoreError::NotFound(_)) => {}
                    Err(e) => return Err(e.into()),
                }
            }
        }
        Ok(())
    }

    /// Push the overlay plan to each host that has already said hello, when
    /// the plan changed, the agent said hello again (it restarted), or the
    /// last apply failed more than [`OVERLAY_RETRY`] ago. The send is not
    /// awaited here: a background task records a failed answer.
    async fn sync_overlay(&self) {
        if !crate::overlay::enabled() {
            return;
        }
        let Some(queue) = &self.queue else {
            return;
        };
        let plans = match crate::overlay::all_plans(&self.store) {
            Ok(plans) => plans,
            Err(e) => {
                tracing::warn!(error = %e, "forge: overlay plan was not built");
                return;
            }
        };
        let known: BTreeMap<String, Option<chrono::DateTime<Utc>>> = queue
            .nodes()
            .into_iter()
            .map(|n| (n.name, n.last_hello))
            .collect();
        for (node, plan) in plans {
            let Some(&hello) = known.get(&node) else {
                continue;
            };
            let body = match serde_json::to_string(&plan) {
                Ok(body) => body,
                Err(e) => {
                    tracing::warn!(node, error = %e, "forge: overlay plan was not encoded");
                    continue;
                }
            };
            if !overlay_due(self.overlay_sent.lock().get(&node), &body, hello) {
                continue;
            }
            let payload = serde_json::to_value(&plan).unwrap_or(serde_json::Value::Null);
            match queue.enqueue(&node, CommandKind::SyncOverlay, payload) {
                Ok(ticket) => {
                    self.overlay_sent.lock().insert(
                        node.clone(),
                        OverlaySent {
                            body: body.clone(),
                            hello,
                            failed: None,
                        },
                    );
                    let sent = self.overlay_sent.clone();
                    tokio::spawn(async move {
                        let why = match ticket.wait().await {
                            Ok(r) if r.ok => return,
                            Ok(r) => r.message,
                            Err(e) => e.to_string(),
                        };
                        tracing::warn!(node, error = %why, "forge: overlay apply failed; will retry");
                        overlay_failed(&sent, &node, &body);
                    });
                }
                Err(e) => {
                    tracing::warn!(node, error = %e, "forge: overlay plan was not queued");
                }
            }
        }
    }

    /// Heartbeat local hosts, mark stale ones, recompute free. Returns the
    /// post-update host list.
    fn reconcile_hosts(
        &self,
        machines: &[Machine],
        size_of: &(dyn Fn(&Machine) -> Option<Resources> + Sync),
    ) -> anyhow::Result<Vec<Host>> {
        let col = self.hosts();
        let now = Utc::now();
        let stale = chrono::Duration::from_std(HOST_STALE_AFTER)?;

        let mut used: BTreeMap<Id, Resources> = BTreeMap::new();
        for m in machines {
            if let (Some(h), Some(r)) = (&m.status.host, size_of(m)) {
                let cur = used.entry(h.clone()).or_default();
                *cur = cur.add(&r);
            }
        }

        let mut out = Vec::new();
        for mut h in col.list(None)? {
            let before = h.status.clone();
            if self.drivers.is_local(&h.meta.id) {
                h.status.last_seen = Some(now);
            }
            let unseen = match h.status.last_seen {
                Some(t) => now - t > stale,
                None => true,
            };
            if unseen {
                if h.status.phase != Phase::Error {
                    h.status.phase = Phase::Error;
                    h.status.message = format!("no report for {}s", HOST_STALE_AFTER.as_secs());
                }
            } else if h.status.phase == Phase::Error && h.status.message.starts_with("no report") {
                h.status.phase = Phase::Ready;
                h.status.message.clear();
            }
            h.status.free = h
                .spec
                .capacity
                .saturating_sub(&used.get(&h.meta.id).copied().unwrap_or_default());
            if h.status != before {
                h = match col.update(h, None) {
                    Ok(h) => h,
                    Err(StoreError::NotFound(_)) => continue,
                    Err(e) => return Err(e.into()),
                };
            }
            out.push(h);
        }
        Ok(out)
    }

    async fn converge(
        &self,
        mut m: Machine,
        hosts: &[Host],
        alloc: &mut Allocations,
        size_of: &(dyn Fn(&Machine) -> Option<Resources> + Sync),
    ) -> anyhow::Result<()> {
        // Host reachability condition for anything already placed.
        if let Some(hid) = &m.status.host {
            let reachable = hosts
                .iter()
                .any(|h| &h.meta.id == hid && h.status.phase == Phase::Ready);
            let msg = if reachable {
                String::new()
            } else {
                format!("host {hid} is not ready")
            };
            m = self
                .patch(&m.meta.id, |s| {
                    s.set_condition(
                        "HostReachable",
                        reachable,
                        if reachable { "Ready" } else { "HostError" },
                        &msg,
                    );
                })
                .await?;
            if !reachable && m.status.phase != Phase::Pending {
                return Ok(());
            }
        }

        match m.status.phase {
            Phase::Pending => {
                if let Some(msg) = volume_wait(&m) {
                    let msg = msg.to_string();
                    self.patch(&m.meta.id, |s| {
                        s.message = msg.clone();
                        s.set_condition("Running", false, "VolumeNotAttached", &msg);
                    })
                    .await?;
                    return Ok(());
                }
                let Some(size) = size_of(&m) else {
                    self.patch(&m.meta.id, |s| {
                        s.phase = Phase::Error;
                        s.message = format!("size '{}' not found", m.spec.size);
                    })
                    .await?;
                    return Ok(());
                };
                let named_network = m.spec.networks.iter().any(|n| !n.trim().is_empty());
                let pinned = pinned_hosts(&m, hosts);
                let candidates = pinned.as_deref().unwrap_or(hosts);
                let decided = if named_network && crate::ovn::enabled() {
                    scheduler::place_on_ovn(&m.spec, &size, candidates, alloc, self.strategy)
                } else if named_network {
                    scheduler::place_on_segment(&m.spec, &size, candidates, alloc, self.strategy)
                } else {
                    scheduler::place(&m.spec, &size, candidates, alloc, self.strategy)
                };
                let host = match decided {
                    Ok(h) => h,
                    Err(e) => {
                        let msg = e.message.clone();
                        self.patch(&m.meta.id, |s| {
                            s.set_condition("Scheduled", false, "NoHost", &msg);
                            s.message = msg.clone();
                        })
                        .await?;
                        return Ok(());
                    }
                };
                alloc.add(&host, &m.meta.name, size);
                let m = self
                    .patch(&m.meta.id, |s| {
                        s.host = Some(host.clone());
                        s.phase = Phase::Creating;
                        s.message.clear();
                        s.set_condition("Scheduled", true, "Placed", &format!("placed on {host}"));
                    })
                    .await?;
                self.start(m).await
            }
            Phase::Creating => self.start(m).await,
            Phase::Ready => self.apply_power(m).await,
            Phase::Error => {
                // A reboot request is the user's "try again".
                if m.spec.restarts > m.status.restarts && m.status.host.is_some() {
                    let n = m.spec.restarts;
                    let m = self
                        .patch(&m.meta.id, |s| {
                            s.restarts = n;
                            s.phase = Phase::Creating;
                            s.message.clear();
                        })
                        .await?;
                    return self.start(m).await;
                }
                Ok(())
            }
            Phase::Updating | Phase::Deleting => Ok(()),
        }
    }

    fn driver_for(&self, m: &Machine) -> Result<(Id, Arc<dyn HostDriver>), ApiError> {
        let hid = m
            .status
            .host
            .clone()
            .ok_or_else(|| ApiError::internal(format!("machine {} has no host", m.meta.id)))?;
        let d = self
            .drivers
            .get(&hid)
            .ok_or_else(|| ApiError::unavailable(format!("no driver registered for host {hid}")))?;
        Ok((hid, d))
    }

    async fn start(&self, m: Machine) -> anyhow::Result<()> {
        // Loom holds the machine here until the slab volume has a host path.
        // Starting earlier creates the container before the bind source exists.
        if let Some(msg) = volume_wait(&m) {
            let msg = msg.to_string();
            self.patch(&m.meta.id, |s| {
                s.message = msg.clone();
                s.set_condition("Running", false, "VolumeNotAttached", &msg);
            })
            .await?;
            return Ok(());
        }
        let (hid, driver) = match self.driver_for(&m) {
            Ok(x) => x,
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        };
        if let Err(e) = crate::nic::ensure_ports(&self.store, &m).await {
            tracing::warn!(machine = %m.meta.id, error = %e, "forge: port was not allocated");
            let msg = e.message.clone();
            self.patch(&m.meta.id, |s| {
                s.phase = Phase::Error;
                s.message = msg.clone();
                s.set_condition("Running", false, "StartFailed", &msg);
            })
            .await?;
            return Ok(());
        }
        match driver.start(&m).await {
            Ok(rt) => {
                tracing::info!(machine = %m.meta.id, name = %m.meta.name, host = %hid, addrs = ?rt.addresses, "forge: machine ready");
                let network_message = rt.message.clone();
                self.patch(&m.meta.id, |s| {
                    s.phase = Phase::Ready;
                    s.addresses = rt.addresses.clone();
                    s.power = rt.power;
                    s.message.clear();
                    s.set_condition("Running", true, "Started", "");
                    note_network(s, &network_message);
                })
                .await?;
            }
            Err(e) if e.code == ErrorCode::Unavailable && e.message.contains("no segment") => {
                tracing::info!(machine = %m.meta.id, error = %e, "forge: waiting for the network segment");
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| {
                    s.message = msg.clone();
                    s.set_condition("Scheduled", true, "Segment", &msg);
                })
                .await?;
            }
            Err(e) => {
                tracing::warn!(machine = %m.meta.id, host = %hid, error = %e, "forge: driver start failed");
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| {
                    s.phase = Phase::Error;
                    s.message = msg.clone();
                    s.set_condition("Running", false, "StartFailed", &msg);
                })
                .await?;
            }
        }
        Ok(())
    }

    async fn apply_power(&self, m: Machine) -> anyhow::Result<()> {
        let want_restart = m.spec.restarts > m.status.restarts;
        let want_off = m.spec.power == DesiredPower::Off;
        let is_on = m.status.power == PowerState::On;
        if !want_restart && want_off != is_on {
            return Ok(());
        }
        let (_, driver) = match self.driver_for(&m) {
            Ok(x) => x,
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        };
        let restarts = m.spec.restarts;
        let result: Result<MachineStatus, ApiError> = async {
            let mut st = m.status.clone();
            if want_restart || want_off {
                if is_on {
                    driver.stop(&m.meta.id).await?;
                }
                st.power = PowerState::Off;
            }
            let network_message = if !want_off {
                crate::nic::ensure_ports(&self.store, &m).await?;
                let rt = driver.start(&m).await?;
                st.power = rt.power;
                st.addresses = rt.addresses;
                rt.message
            } else {
                String::new()
            };
            st.restarts = restarts;
            st.message.clear();
            note_network(&mut st, &network_message);
            Ok(st)
        }
        .await;
        match result {
            Ok(st) => {
                self.patch(&m.meta.id, |s| {
                    s.power = st.power;
                    s.addresses = st.addresses.clone();
                    s.restarts = st.restarts;
                    s.message = st.message.clone();
                    s.set_condition("Running", st.power == PowerState::On, "PowerApplied", "");
                    copy_network(s, &st);
                })
                .await?;
            }
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
            }
        }
        Ok(())
    }

    async fn teardown(&self, m: Machine) -> anyhow::Result<()> {
        let m = if m.status.phase != Phase::Deleting {
            self.patch(&m.meta.id, |s| {
                s.phase = Phase::Deleting;
                s.message.clear();
            })
            .await?
        } else {
            m
        };
        if m.status.host.is_some() {
            let (hid, driver) = match self.driver_for(&m) {
                Ok(x) => x,
                Err(e) => {
                    let msg = e.message.clone();
                    self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                    return Ok(());
                }
            };
            if let Err(e) = driver.stop(&m.meta.id).await {
                if e.code != pistack_types::error::ErrorCode::NotFound {
                    tracing::debug!(machine = %m.meta.id, error = %e, "forge: stop before delete failed");
                }
            }
            if let Err(e) = driver.delete(&m.meta.id).await {
                let msg = e.message.clone();
                tracing::warn!(machine = %m.meta.id, host = %hid, error = %e, "forge: driver delete failed");
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        }
        if let Err(e) = crate::nic::release_ports(&self.store, &m.meta.id) {
            tracing::warn!(machine = %m.meta.id, error = %e, "forge: port release failed");
        }
        match self.machines().delete(&m.meta.id) {
            Ok(_) => {
                tracing::info!(machine = %m.meta.id, name = %m.meta.name, "forge: machine deleted")
            }
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        Ok(())
    }

    /// Apply `f` to the machine's status and write it back only if it
    /// changed. Returns the stored machine.
    async fn patch(&self, id: &Id, f: impl Fn(&mut MachineStatus)) -> Result<Machine, StoreError> {
        let col = self.machines();
        for _ in 0..8 {
            let cur = col.get(id)?;
            let mut next = cur.clone();
            f(&mut next.status);
            if next.status == cur.status {
                return Ok(cur);
            }
            match col.update(next, Some(cur.meta.revision)) {
                Ok(m) => return Ok(m),
                Err(StoreError::Revision { .. }) => continue,
                Err(e) => return Err(e),
            }
        }
        Err(StoreError::Conflict(format!(
            "machine {id}: too many concurrent writers"
        )))
    }
}

fn nonempty_annotation<'a>(m: &'a Machine, key: &str) -> Option<&'a str> {
    m.meta
        .annotations
        .get(key)
        .map(String::as_str)
        .filter(|value| !value.is_empty())
}

fn volume_wait(m: &Machine) -> Option<&str> {
    nonempty_annotation(m, WAIT_VOLUME_ANNOTATION)
}

fn pinned_hosts(m: &Machine, hosts: &[Host]) -> Option<Vec<Host>> {
    let id = nonempty_annotation(m, NODE_PIN_ANNOTATION)?;
    Some(
        hosts
            .iter()
            .filter(|h| h.meta.id.to_string() == id)
            .cloned()
            .collect(),
    )
}

/// A host that could not attach the recorded address says so here.
/// An empty message drops the condition. The Running condition stays.
fn note_network(status: &mut MachineStatus, message: &str) {
    if message.is_empty() {
        status.conditions.retain(|c| c.kind != "Network");
        return;
    }
    status.message = message.to_owned();
    status.set_condition("Network", false, "Rootless", message);
}

fn copy_network(dest: &mut MachineStatus, src: &MachineStatus) {
    if let Some(net) = src.conditions.iter().find(|c| c.kind == "Network") {
        dest.set_condition("Network", net.status, &net.reason, &net.message);
    } else {
        dest.conditions.retain(|c| c.kind != "Network");
    }
}

#[cfg(test)]
mod pin_tests {
    use super::{pinned_hosts, volume_wait};
    use crate::types::{
        new_host, new_machine, MachineSpec, NODE_PIN_ANNOTATION, WAIT_VOLUME_ANNOTATION,
    };
    use pistack_types::Resources;

    #[test]
    fn wait_annotation_is_a_message_and_a_pin_keeps_one_host() {
        let mut machine = new_machine("pod-web", None, MachineSpec::default());
        assert!(volume_wait(&machine).is_none());
        machine.meta.annotations.insert(
            WAIT_VOLUME_ANNOTATION.into(),
            "waiting for volume data".into(),
        );
        assert_eq!(volume_wait(&machine), Some("waiting for volume data"));
        machine
            .meta
            .annotations
            .insert(WAIT_VOLUME_ANNOTATION.into(), String::new());
        assert!(volume_wait(&machine).is_none());

        let cap = Resources {
            vcpus: 1,
            memory_mib: 512,
            ..Resources::default()
        };
        let a = new_host("a", "a", cap, vec!["youki".into()], Default::default());
        let b = new_host("b", "b", cap, vec!["youki".into()], Default::default());
        let hosts = vec![a.clone(), b];
        assert!(pinned_hosts(&machine, &hosts).is_none());
        machine
            .meta
            .annotations
            .insert(NODE_PIN_ANNOTATION.into(), a.meta.id.to_string());
        let pinned = pinned_hosts(&machine, &hosts).unwrap();
        assert_eq!(pinned.len(), 1);
        assert_eq!(pinned[0].meta.id, a.meta.id);
    }

    #[test]
    fn a_network_note_is_a_false_condition_and_clears() {
        let mut machine = new_machine("pod-web", None, MachineSpec::default());
        let message = "pod has no network: a rootless agent cannot attach the recorded address. Run the agent as root or set PISTACK_YOUKI_SUDO=1";
        super::note_network(&mut machine.status, message);
        assert_eq!(machine.status.message, message);
        let condition = machine
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Network")
            .unwrap();
        assert!(!condition.status);
        assert_eq!(condition.reason, "Rootless");
        assert_eq!(condition.message, message);
        super::note_network(&mut machine.status, "");
        assert!(machine
            .status
            .conditions
            .iter()
            .all(|c| c.kind != "Network"));
    }
}

#[cfg(test)]
mod overlay_retry_tests {
    use super::{overlay_due, overlay_failed, OverlaySent, OVERLAY_RETRY};
    use chrono::Utc;
    use std::collections::BTreeMap;
    use std::sync::Arc;
    use std::time::Instant;

    #[test]
    fn a_plan_goes_again_on_change_new_hello_or_old_failure() {
        let hello = Some(Utc::now());
        assert!(overlay_due(None, "a", hello));
        let sent = OverlaySent {
            body: "a".into(),
            hello,
            failed: None,
        };
        // Same plan, same agent, no failure: nothing to do.
        assert!(!overlay_due(Some(&sent), "a", hello));
        // The plan changed.
        assert!(overlay_due(Some(&sent), "b", hello));
        // The agent restarted and said hello again.
        let later = Some(Utc::now() + chrono::Duration::seconds(5));
        assert!(overlay_due(Some(&sent), "a", later));
        // A fresh failure waits; an old one goes again.
        let fresh = OverlaySent {
            failed: Some(Instant::now()),
            ..sent.clone()
        };
        assert!(!overlay_due(Some(&fresh), "a", hello));
        if let Some(old) = Instant::now().checked_sub(OVERLAY_RETRY) {
            let old = OverlaySent {
                failed: Some(old),
                ..sent
            };
            assert!(overlay_due(Some(&old), "a", hello));
        }
    }

    #[tokio::test]
    async fn taking_the_lead_forgets_what_was_sent() {
        let store = pistack_store::Store::in_memory().unwrap();
        let drivers = Arc::new(crate::driver::DriverRegistry::new());
        let r = super::Reconciler::new(store, drivers, crate::scheduler::Strategy::default());
        r.overlay_sent.lock().insert(
            "sim2".to_owned(),
            OverlaySent {
                body: "a".into(),
                hello: None,
                failed: None,
            },
        );
        let active = pistack_types::CancellationToken::new();
        active.cancel();
        r.run_loop(active).await;
        assert!(r.overlay_sent.lock().is_empty());
    }

    #[test]
    fn a_failure_on_an_older_plan_does_not_mark_the_newer_one() {
        let map = Arc::new(parking_lot::Mutex::new(BTreeMap::new()));
        map.lock().insert(
            "sim2".to_owned(),
            OverlaySent {
                body: "new".into(),
                hello: None,
                failed: None,
            },
        );
        overlay_failed(&map, "sim2", "old");
        assert!(map.lock()["sim2"].failed.is_none());
        overlay_failed(&map, "sim2", "new");
        assert!(map.lock()["sim2"].failed.is_some());
    }
}
