// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The reconciler: turns `spec` into backend state and writes `status`.
//!
//! Driven by store events (`handle_event`), a full pass at start
//! (`reconcile_all`) and a periodic retry of anything not `Ready`
//! (`reconcile_pending`). Every status write records
//! `observed_revision = revision + 1`, the revision the write itself
//! produces, so the event that write emits is recognised and skipped.
//!
//! Dependencies are pulled, not scheduled: realising a port first makes
//! sure its network is realised, a router makes sure its ports are. That
//! keeps ordering correct whatever order events arrive in.

use crate::backend::{FipView, RouterView};
use crate::client::{Client, PortRef};
use crate::types::*;
use pistack_store::{Collection, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Event, EventAction, Id, Kind, Phase};
use std::net::IpAddr;

/// Common status surface, so the event loop can ask "is this stale?" for
/// any of our kinds.
pub trait Observed: Kind {
    fn phase(&self) -> Phase;
    fn observed_revision(&self) -> u64;
    fn message(&self) -> &str;
    fn set_result(&mut self, phase: Phase, message: String, observed: u64);
    /// Written since the reconciler last looked. An event acts on this.
    fn unseen(&self) -> bool {
        self.observed_revision() != self.meta().revision
    }
    /// Due on the periodic pass: unseen, or failed last time. A failure is
    /// not retried on events. The status write that records it is itself an
    /// event, so retrying there never stops.
    fn stale(&self) -> bool {
        self.unseen() || self.phase() == Phase::Error
    }
}

macro_rules! observed {
    ($t:ty) => {
        impl Observed for $t {
            fn phase(&self) -> Phase {
                self.status.phase
            }
            fn observed_revision(&self) -> u64 {
                self.status.observed_revision
            }
            fn message(&self) -> &str {
                &self.status.message
            }
            fn set_result(&mut self, phase: Phase, message: String, observed: u64) {
                self.status.phase = phase;
                self.status.message = message;
                self.status.observed_revision = observed;
            }
        }
    };
}
observed!(Network);
observed!(Subnet);
observed!(Port);
observed!(Router);
observed!(SecurityGroup);
observed!(FloatingIp);

/// Write a status outcome. `extra` runs before the phase is set and may
/// fill kind-specific status fields.
///
/// `acted` is the revision this pass read. A spec write during the backend
/// call must stay stale: the event loop handles one event at a time, so
/// that write's event is still queued, and claiming it here would skip the
/// backend work for the new spec.
fn finish<K: Observed>(
    col: &Collection<K>,
    id: &Id,
    acted: u64,
    outcome: ApiResult<()>,
    mut extra: impl FnMut(&mut K),
) -> Phase {
    if matches!(&outcome, Err(e) if e.message == "not the leader") {
        tracing::debug!(kind = K::KIND, %id, "reconcile stopped; not the leader");
        return Phase::Updating;
    }
    let (phase, msg) = match &outcome {
        Ok(()) => (Phase::Ready, String::new()),
        Err(e) => (Phase::Error, e.to_string()),
    };
    // A retry that fails the same way has nothing new to say. Writing it
    // again would add a log entry per failed resource on every pass.
    if phase == Phase::Error {
        if let Ok(cur) = col.get(id) {
            if cur.meta().revision == acted
                && !cur.unseen()
                && cur.phase() == Phase::Error
                && cur.message() == msg
            {
                tracing::debug!(kind = K::KIND, %id, error = %msg, "reconcile still failing");
                return phase;
            }
        }
    }
    let res = col.modify(id, |cur| {
        if cur.meta().revision != acted {
            return Err(StoreError::Conflict("reconcile superseded".into()));
        }
        extra(cur);
        let next = cur.meta().revision + 1;
        cur.set_result(phase, msg.clone(), next);
        Ok(())
    });
    match res {
        Ok(_) => {
            if phase == Phase::Error {
                pistack_types::tally::reconcile_failed("fabric");
                tracing::warn!(kind = K::KIND, %id, error = %msg, "reconcile failed");
            } else {
                tracing::debug!(kind = K::KIND, %id, "reconciled");
            }
            phase
        }
        // Deleted while we worked; the Deleted event cleans up.
        Err(StoreError::NotFound(_)) => phase,
        Err(StoreError::NotLeader) => {
            tracing::debug!(kind = K::KIND, %id, "status write skipped; not the leader");
            Phase::Updating
        }
        Err(StoreError::Conflict(reason)) if reason == "reconcile superseded" => {
            tracing::debug!(kind = K::KIND, %id, "spec changed during reconcile");
            Phase::Updating
        }
        Err(e) => {
            tracing::error!(kind = K::KIND, %id, error = %e, "status write failed");
            phase
        }
    }
}

fn api(e: StoreError) -> ApiError {
    e.into()
}

impl Client {
    /// React to one store event. Ignores kinds fabric does not own and
    /// revisions already observed, a recorded failure among them: the
    /// periodic pass retries those.
    pub async fn handle_event(&self, ev: &Event) {
        let k = ev.kind.as_str();
        if !KINDS.contains(&k) {
            return;
        }
        if ev.action == EventAction::Deleted {
            self.handle_deleted(k, &ev.id).await;
            return;
        }
        let unseen = match k {
            kind::NETWORK => self.is_unseen(&self.networks(), &ev.id),
            kind::SUBNET => self.is_unseen(&self.subnets(), &ev.id),
            kind::PORT => self.is_unseen(&self.ports(), &ev.id),
            kind::ROUTER => self.is_unseen(&self.routers(), &ev.id),
            kind::SECGROUP => self.is_unseen(&self.security_groups(), &ev.id),
            kind::FLOATING_IP => self.is_unseen(&self.floating_ips(), &ev.id),
            _ => false,
        };
        if !unseen {
            return;
        }
        match k {
            kind::NETWORK => {
                self.reconcile_network(&ev.id).await;
            }
            kind::SUBNET => {
                self.reconcile_subnet(&ev.id).await;
            }
            kind::PORT => {
                self.reconcile_port(&ev.id).await;
            }
            kind::ROUTER => {
                self.reconcile_router(&ev.id).await;
            }
            kind::SECGROUP => {
                self.reconcile_security_group(&ev.id).await;
            }
            kind::FLOATING_IP => {
                self.reconcile_floating_ip(&ev.id).await;
            }
            _ => {}
        }
    }

    fn is_unseen<K: Observed>(&self, col: &Collection<K>, id: &Id) -> bool {
        match col.get(id) {
            Ok(r) => r.unseen(),
            Err(_) => false,
        }
    }

    async fn handle_deleted(&self, k: &str, id: &Id) {
        let b = self.backend();
        let res = match k {
            kind::NETWORK => b.delete_network(id).await,
            kind::SUBNET => b.delete_subnet(id).await,
            kind::PORT => {
                let r = b.delete_port(id).await;
                let remembered = self.port_index.lock().remove(id);
                if let Some(pr) = remembered {
                    if let Some(s) = &pr.subnet {
                        self.refresh_subnet_allocated(s);
                    }
                    for g in &pr.groups {
                        self.reconcile_security_group(g).await;
                    }
                }
                r
            }
            kind::ROUTER => b.delete_router(id).await,
            kind::SECGROUP => b.delete_security_group(id).await,
            kind::FLOATING_IP => b.delete_fip(id).await,
            _ => Ok(()),
        };
        if let Err(e) = res {
            tracing::warn!(kind = k, %id, error = %e, "backend delete failed");
        }
    }

    /// Realise every resource, in dependency order. Used at start.
    pub async fn reconcile_all(&self) {
        self.pass(true).await;
    }

    /// Realise only what is not `Ready` or has an unobserved revision.
    pub async fn reconcile_pending(&self) {
        self.pass(false).await;
    }

    async fn pass(&self, force: bool) {
        macro_rules! each {
            ($col:expr, $f:ident) => {
                match $col.list(None) {
                    Ok(items) => {
                        for r in items {
                            if force || r.stale() {
                                self.$f(&r.meta.id).await;
                            }
                        }
                    }
                    Err(e) => tracing::error!(error = %e, "list failed during reconcile pass"),
                }
            };
        }
        each!(self.networks(), reconcile_network);
        each!(self.subnets(), reconcile_subnet);
        each!(self.security_groups(), reconcile_security_group);
        each!(self.ports(), reconcile_port);
        each!(self.routers(), reconcile_router);
        each!(self.floating_ips(), reconcile_floating_ip);
    }

    // ---- per kind ---------------------------------------------------------

    pub async fn reconcile_network(&self, id: &Id) -> Phase {
        let col = self.networks();
        let Ok(net) = col.get(id) else {
            return Phase::Deleting;
        };
        let segment = match &net.spec.kind {
            NetworkKind::Provider { vlan: Some(v), .. } => Some(*v as u32),
            NetworkKind::Provider { vlan: None, .. } => None,
            NetworkKind::Overlay => match net.status.segment_id {
                Some(s) => Some(s),
                None => self.next_segment().ok(),
            },
        };
        let mut net = net;
        if let Some(s) = segment {
            net.status.segment_id = Some(s);
        }
        let datapath = self.stick_datapath(&net);
        let outcome = self.backend().ensure_network(&net).await;
        finish(&col, id, net.meta.revision, outcome, |n| {
            n.status.segment_id = segment;
            if n.status.datapath.is_none() {
                if let Some(path) = datapath.clone() {
                    n.status.datapath = Some(path);
                }
            }
        })
    }

    /// First realisation wins. A guest port already bound `hostnat`, the
    /// shared 10.88.0.0/24 network, or a host-NAT backend stays on that
    /// bridge. The choice is not rewritten when the backend flag changes.
    fn stick_datapath(&self, net: &Network) -> Option<String> {
        if net.status.datapath.is_some() {
            return None;
        }
        let shared = self
            .subnets()
            .list(None)
            .unwrap_or_default()
            .iter()
            .any(|s| s.spec.network == net.meta.id && s.spec.cidr.to_string() == "10.88.0.0/24");
        let hostnat_port = self.ports().list(None).unwrap_or_default().iter().any(|p| {
            p.spec.network == net.meta.id
                && !p.is_router_port()
                && p.status
                    .binding
                    .as_ref()
                    .is_some_and(|b| b.vif_type == "hostnat")
        });
        if shared || hostnat_port || self.backend().name() == "hostnat" {
            return Some("hostnat".into());
        }
        if self.backend().name() == "ovn" {
            return Some("ovn".into());
        }
        if self.backend().name() == "overlay" && matches!(net.spec.kind, NetworkKind::Overlay) {
            return Some("overlay".into());
        }
        None
    }

    /// Overlay segment ids are VXLAN VNIs from a store counter. The backend
    /// is given the segment before ensure.
    fn next_segment(&self) -> ApiResult<u32> {
        const KEY: &str = "fabric/next_segment";
        let cur: u32 = self.store().get_meta(KEY).map_err(api)?.unwrap_or(1);
        self.store().put_meta(KEY, &(cur + 1)).map_err(api)?;
        Ok(cur)
    }

    /// Make sure a network is realised; returns it when Ready.
    async fn ready_network(&self, id: &Id) -> ApiResult<Network> {
        let net = self.networks().get(id).map_err(api)?;
        if net.status.phase == Phase::Ready && !net.stale() {
            return Ok(net);
        }
        if self.reconcile_network(id).await != Phase::Ready {
            return Err(ApiError::unavailable(format!("network {id} is not ready")));
        }
        self.networks().get(id).map_err(api)
    }

    pub async fn reconcile_subnet(&self, id: &Id) -> Phase {
        let col = self.subnets();
        let Ok(sub) = col.get(id) else {
            return Phase::Deleting;
        };
        let outcome = async {
            let net = self.ready_network(&sub.spec.network).await?;
            self.backend().ensure_subnet(&net, &sub).await
        }
        .await;
        let allocated = self
            .used_addresses(&sub)
            .map(|u| u.len() as u32)
            .unwrap_or(sub.status.allocated);
        finish(&col, id, sub.meta.revision, outcome, |s| {
            s.status.allocated = allocated
        })
    }

    /// Recount a subnet's held addresses without touching the backend.
    pub(crate) fn refresh_subnet_allocated(&self, id: &Id) {
        let col = self.subnets();
        let Ok(sub) = col.get(id) else { return };
        let Ok(used) = self.used_addresses(&sub) else {
            return;
        };
        let n = used.len() as u32;
        if n == sub.status.allocated {
            return;
        }
        let _ = col.modify(id, |s| {
            s.status.allocated = n;
            s.status.observed_revision = s.meta.revision + 1;
            Ok(())
        });
    }

    pub async fn reconcile_port(&self, id: &Id) -> Phase {
        let col = self.ports();
        let Ok(port) = col.get(id) else {
            return Phase::Deleting;
        };
        let outcome = async {
            let net = self.ready_network(&port.spec.network).await?;
            let mut groups = Vec::new();
            for g in &port.spec.security_groups {
                groups.push(self.security_groups().get(g).map_err(api)?);
            }
            self.backend().ensure_port(&net, &port, &groups).await
        }
        .await;
        let (res, binding) = match outcome {
            Ok(b) => (Ok(()), Some(b)),
            Err(e) => (Err(e), None),
        };
        let phase = finish(&col, id, port.meta.revision, res, |p| {
            if binding.is_some() {
                p.status.binding = binding.clone();
            }
        });
        self.port_index.lock().insert(
            id.clone(),
            PortRef {
                subnet: port.spec.subnet.clone(),
                groups: port.spec.security_groups.clone(),
            },
        );
        if let Some(s) = &port.spec.subnet {
            self.refresh_subnet_allocated(s);
        }
        // Membership changed: push the new member list to each group.
        for g in &port.spec.security_groups {
            self.reconcile_security_group(g).await;
        }
        phase
    }

    pub async fn reconcile_router(&self, id: &Id) -> Phase {
        let col = self.routers();
        let Ok(router) = col.get(id) else {
            return Phase::Deleting;
        };
        let mut gateway_port = None;
        let mut external_ip = None;
        let outcome = async {
            let ports = self.ports_of_device(id)?;
            // Router ports must exist on the backend before the LRPs reference them.
            for p in &ports {
                if self.reconcile_port(&p.meta.id).await != Phase::Ready {
                    return Err(ApiError::unavailable(format!(
                        "router port {} is not ready",
                        p.meta.id
                    )));
                }
            }
            let gateway = match &router.spec.external_network {
                Some(n) => {
                    let net = self.networks().get(n).map_err(api)?;
                    let port = ports
                        .iter()
                        .find(|p| &p.spec.network == n)
                        .cloned()
                        .ok_or_else(|| {
                            ApiError::internal(format!("router {id} has no gateway port on {n}"))
                        })?;
                    gateway_port = Some(port.meta.id.clone());
                    external_ip = port.spec.fixed_ips.first().copied();
                    Some((net, port))
                }
                None => None,
            };
            let gateway_subnet = match &gateway {
                Some((_, port)) => match &port.spec.subnet {
                    Some(sid) => Some(self.subnets().get(sid).map_err(api)?),
                    None => None,
                },
                None => None,
            };
            let mut interfaces = Vec::new();
            for sid in &router.spec.interfaces {
                let sub = self.subnets().get(sid).map_err(api)?;
                let port = ports
                    .iter()
                    .find(|p| p.spec.subnet.as_ref() == Some(sid))
                    .cloned()
                    .ok_or_else(|| {
                        ApiError::internal(format!("router {id} has no interface port on {sid}"))
                    })?;
                interfaces.push((sub, port));
            }
            let view = RouterView {
                router: router.clone(),
                gateway,
                gateway_subnet,
                interfaces,
            };
            self.backend().ensure_router(&view).await
        }
        .await;
        finish(&col, id, router.meta.revision, outcome, |r| {
            r.status.gateway_port = gateway_port.clone();
            r.status.external_ip = external_ip;
        })
    }

    pub async fn reconcile_security_group(&self, id: &Id) -> Phase {
        let col = self.security_groups();
        let Ok(sg) = col.get(id) else {
            return Phase::Deleting;
        };
        let members: Vec<Port> = match self.ports().list(None) {
            Ok(ps) => ps
                .into_iter()
                .filter(|p| p.spec.security_groups.contains(id))
                .collect(),
            Err(e) => return finish(&col, id, sg.meta.revision, Err(api(e)), |_| {}),
        };
        let n = members.len() as u32;
        let outcome = self.backend().apply_security_group(&sg, &members).await;
        finish(&col, id, sg.meta.revision, outcome, |s| {
            s.status.members = n
        })
    }

    pub async fn reconcile_floating_ip(&self, id: &Id) -> Phase {
        let col = self.floating_ips();
        let Ok(fip) = col.get(id) else {
            return Phase::Deleting;
        };
        let mut fixed_ip: Option<IpAddr> = None;
        let mut router_id: Option<Id> = None;
        let outcome = async {
            let target = match &fip.spec.port {
                None => None,
                Some(pid) => {
                    let port = self.ports().get(pid).map_err(api)?;
                    let want_v4 = fip.spec.address.map(|a| a.is_ipv4()).unwrap_or(true);
                    let ip = port
                        .spec
                        .fixed_ips
                        .iter()
                        .copied()
                        .find(|ip| ip.is_ipv4() == want_v4)
                        .ok_or_else(|| {
                            ApiError::unprocessable(format!(
                                "port {pid} has no fixed ip of the right family"
                            ))
                        })?;
                    fixed_ip = Some(ip);
                    Some((port, ip))
                }
            };
            // A router is how OVN will carry the NAT. Host NAT does not need
            // one: the hypervisor is the router, and the fixed IP is enough.
            let router = match &target {
                None => None,
                Some((port, _)) => {
                    let found = port.spec.subnet.as_ref().and_then(|sid| {
                        self.routers().list(None).ok()?.into_iter().find(|r| {
                            r.spec.interfaces.contains(sid)
                                && r.spec.external_network.as_ref() == Some(&fip.spec.network)
                        })
                    });
                    if let Some(r) = &found {
                        router_id = Some(r.meta.id.clone());
                    }
                    found
                }
            };
            let view = FipView {
                fip: fip.clone(),
                target,
                router,
            };
            self.backend().associate_fip(&view).await
        }
        .await;
        finish(&col, id, fip.meta.revision, outcome, |f| {
            f.status.fixed_ip = fixed_ip;
            f.status.router = router_id.clone();
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{FipView, NetBackend, RouterView};
    use crate::Client;
    use async_trait::async_trait;
    use pistack_store::Store;
    use pistack_types::Meta;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use tokio::sync::Notify;

    /// Holds the first floating-IP call so a spec write can land underneath it.
    struct Gate {
        entered: Arc<Notify>,
        release: Arc<Notify>,
        held: AtomicBool,
        calls: parking_lot::Mutex<Vec<Option<Id>>>,
    }

    #[async_trait]
    impl NetBackend for Gate {
        fn name(&self) -> &'static str {
            "gate"
        }
        async fn ensure_network(&self, _: &Network) -> ApiResult<()> {
            Ok(())
        }
        async fn delete_network(&self, _: &Id) -> ApiResult<()> {
            Ok(())
        }
        async fn ensure_subnet(&self, _: &Network, _: &Subnet) -> ApiResult<()> {
            Ok(())
        }
        async fn delete_subnet(&self, _: &Id) -> ApiResult<()> {
            Ok(())
        }
        async fn ensure_port(
            &self,
            _: &Network,
            _: &Port,
            _: &[SecurityGroup],
        ) -> ApiResult<PortBinding> {
            Ok(PortBinding::default())
        }
        async fn delete_port(&self, _: &Id) -> ApiResult<()> {
            Ok(())
        }
        async fn ensure_router(&self, _: &RouterView) -> ApiResult<()> {
            Ok(())
        }
        async fn delete_router(&self, _: &Id) -> ApiResult<()> {
            Ok(())
        }
        async fn apply_security_group(&self, _: &SecurityGroup, _: &[Port]) -> ApiResult<()> {
            Ok(())
        }
        async fn delete_security_group(&self, _: &Id) -> ApiResult<()> {
            Ok(())
        }
        async fn associate_fip(&self, view: &FipView) -> ApiResult<()> {
            self.calls.lock().push(view.fip.spec.port.clone());
            if !self.held.swap(true, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(())
        }
        async fn delete_fip(&self, _: &Id) -> ApiResult<()> {
            Ok(())
        }
    }

    #[tokio::test]
    async fn spec_change_during_reconcile_stays_stale() {
        let entered = Arc::new(Notify::new());
        let release = Arc::new(Notify::new());
        let gate = Arc::new(Gate {
            entered: entered.clone(),
            release: release.clone(),
            held: AtomicBool::new(false),
            calls: parking_lot::Mutex::new(Vec::new()),
        });
        let store = Store::in_memory().unwrap();
        let client = Client::new(store, gate.clone());
        client.ensure_default_template().unwrap();
        let project = Id::new(kind::PROJECT);
        let ext = NetworkSpec {
            external: true,
            ..Default::default()
        };
        let net = client
            .create_network(
                Meta::new(kind::NETWORK, "fleet", Some(project.clone())),
                ext,
            )
            .unwrap();
        let sub = client
            .create_subnet(
                Meta::new(kind::SUBNET, "pool", Some(project.clone())),
                SubnetSpec {
                    network: net.meta.id.clone(),
                    cidr: "10.66.66.80/28".parse().unwrap(),
                    gateway: Some("10.66.66.81".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: false,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let guest = client
            .create_network(
                Meta::new(kind::NETWORK, "guest", Some(project.clone())),
                NetworkSpec::default(),
            )
            .unwrap();
        client
            .create_subnet(
                Meta::new(kind::SUBNET, "guest", Some(project.clone())),
                SubnetSpec {
                    network: guest.meta.id.clone(),
                    cidr: "10.88.0.0/24".parse().unwrap(),
                    gateway: Some("10.88.0.1".parse().unwrap()),
                    allocation_pools: vec![],
                    dns_nameservers: vec![],
                    enable_dhcp: true,
                    host_routes: vec![],
                },
            )
            .unwrap();
        let port = client
            .create_port(
                Meta::new(kind::PORT, "demo", Some(project.clone())),
                PortSpec {
                    network: guest.meta.id,
                    fixed_ips: vec!["10.88.0.187".parse().unwrap()],
                    ..PortSpec::default()
                },
            )
            .unwrap();
        let fip = client
            .create_floating_ip(
                Meta::new(kind::FLOATING_IP, "demo", Some(project)),
                FloatingIpSpec {
                    network: net.meta.id,
                    address: None,
                    port: None,
                },
            )
            .unwrap();
        let _ = sub;

        let waiting = entered.notified();
        tokio::pin!(waiting);
        let id = fip.meta.id.clone();
        let worker = client.clone();
        let task = tokio::spawn(async move { worker.reconcile_floating_ip(&id).await });
        waiting.await;
        client
            .associate_floating_ip(&fip.meta.id, Some(port.meta.id.clone()))
            .unwrap();
        release.notify_one();
        let _ = task.await.unwrap();

        let mid = client.floating_ips().get(&fip.meta.id).unwrap();
        assert!(
            mid.stale(),
            "the in-flight pass must not mark the new port observed"
        );
        assert_eq!(mid.spec.port, Some(port.meta.id.clone()));

        client.reconcile_floating_ip(&fip.meta.id).await;
        let done = client.floating_ips().get(&fip.meta.id).unwrap();
        assert_eq!(done.status.phase, Phase::Ready);
        assert_eq!(done.status.fixed_ip, Some("10.88.0.187".parse().unwrap()));
        assert!(!done.stale());
        let calls = gate.calls.lock().clone();
        assert_eq!(calls, vec![None, Some(port.meta.id)]);
    }
}
