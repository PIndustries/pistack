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

use crate::driver::{DriverRegistry, HostDriver};
use crate::scheduler::{self, Allocations, Strategy};
use crate::types::{
    delete_requested, DesiredPower, Host, Machine, MachineStatus, PowerState, Size,
};
use chrono::Utc;
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, ApiError, Id, ModuleContext, Phase, Resources};
use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::broadcast::error::RecvError;

/// How often a pass runs even when nothing changed.
pub const TICK: Duration = Duration::from_secs(2);
/// A host that has not reported for this long is `Error`.
pub const HOST_STALE_AFTER: Duration = Duration::from_secs(30);

pub struct Reconciler {
    store: Store,
    drivers: Arc<DriverRegistry>,
    strategy: Strategy,
}

impl Reconciler {
    pub fn new(store: Store, drivers: Arc<DriverRegistry>, strategy: Strategy) -> Self {
        Self {
            store,
            drivers,
            strategy,
        }
    }

    /// Run until `ctx.shutdown` fires.
    pub async fn run(self, ctx: ModuleContext) {
        let mut rx = self.store.events().subscribe();
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => break,
                _ = tick.tick() => {}
                ev = rx.recv() => match ev {
                    Ok(e) if e.kind == kind::MACHINE => {}
                    Ok(_) => continue,
                    Err(RecvError::Lagged(n)) => tracing::debug!(skipped = n, "forge: event bus lagged"),
                    Err(RecvError::Closed) => break,
                },
            }
            if let Err(e) = self.reconcile_once().await {
                tracing::warn!(error = %e, "forge: reconcile pass failed");
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
                tracing::warn!(machine = %id, error = %e, "forge: machine reconcile failed");
            }
        }
        Ok(())
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
                let Some(size) = size_of(&m) else {
                    self.patch(&m.meta.id, |s| {
                        s.phase = Phase::Error;
                        s.message = format!("size '{}' not found", m.spec.size);
                    })
                    .await?;
                    return Ok(());
                };
                let host = match scheduler::place(&m.spec, &size, hosts, alloc, self.strategy) {
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
        let (hid, driver) = match self.driver_for(&m) {
            Ok(x) => x,
            Err(e) => {
                let msg = e.message.clone();
                self.patch(&m.meta.id, |s| s.message = msg.clone()).await?;
                return Ok(());
            }
        };
        match driver.start(&m).await {
            Ok(rt) => {
                tracing::info!(machine = %m.meta.id, name = %m.meta.name, host = %hid, addrs = ?rt.addresses, "forge: machine ready");
                self.patch(&m.meta.id, |s| {
                    s.phase = Phase::Ready;
                    s.addresses = rt.addresses.clone();
                    s.power = rt.power;
                    s.message.clear();
                    s.set_condition("Running", true, "Started", "");
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
        if !want_restart && want_off == !is_on {
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
            if !want_off {
                let rt = driver.start(&m).await?;
                st.power = rt.power;
                st.addresses = rt.addresses;
            }
            st.restarts = restarts;
            st.message.clear();
            Ok(st)
        }
        .await;
        match result {
            Ok(st) => {
                self.patch(&m.meta.id, |s| {
                    s.power = st.power;
                    s.addresses = st.addresses.clone();
                    s.restarts = st.restarts;
                    s.message.clear();
                    s.set_condition("Running", st.power == PowerState::On, "PowerApplied", "");
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
