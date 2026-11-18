// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::backend::{FipView, RouterView};
use crate::client::{Client, PortRef};
use crate::types::*;
use pistack_store::Collection;
use pistack_types::{kind, ApiError, ApiResult, Event, EventAction, Id, Phase};
use std::net::IpAddr;

use super::api::api;
use super::finish::finish;
use super::observed::Observed;
use crate::types::Network;
use crate::types::Port;

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
            kind::VPC => {
                self.reconcile_vpc(&ev.id).await;
            }
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

    pub(crate) fn is_unseen<K: Observed>(&self, col: &Collection<K>, id: &Id) -> bool {
        match col.get(id) {
            Ok(r) => r.unseen(),
            Err(_) => false,
        }
    }

    pub(crate) async fn handle_deleted(&self, k: &str, id: &Id) {
        let b = self.backend();
        let res = match k {
            kind::VPC => Ok(()),
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

    pub(crate) async fn pass(&self, force: bool) {
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
        each!(self.vpcs(), count_vpc_pass);
        each!(self.networks(), reconcile_network);
        each!(self.subnets(), reconcile_subnet);
        each!(self.security_groups(), reconcile_security_group);
        each!(self.ports(), reconcile_port);
        each!(self.routers(), reconcile_router);
        each!(self.floating_ips(), reconcile_floating_ip);
    }

    // ---- per kind ---------------------------------------------------------

    /// A vpc has nothing of its own to realise. The router it owns is
    /// reconciled as a router; a vpc is ready when that router is, and its
    /// subnet count is kept current on the way.
    pub(crate) async fn count_vpc_pass(&self, id: &Id) {
        self.reconcile_vpc(id).await;
    }

    pub async fn reconcile_vpc(&self, id: &Id) -> Phase {
        let col = self.vpcs();
        if col.get(id).is_err() {
            return Phase::Deleting;
        }
        self.count_vpc_subnets(id);
        let v = match col.get(id) {
            Ok(v) => v,
            Err(_) => return Phase::Deleting,
        };
        let outcome = match &v.status.router {
            Some(r) => match self.reconcile_router(r).await {
                Phase::Ready => Ok(()),
                Phase::Error => Err(ApiError::unavailable(format!("router {r} failed"))),
                _ => Err(ApiError::unavailable(format!("router {r} is not ready"))),
            },
            None => Err(ApiError::internal(format!("vpc {id} owns no router"))),
        };
        finish(&col, id, v.meta.revision, outcome, |_| {})
    }

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
    pub(crate) fn stick_datapath(&self, net: &Network) -> Option<String> {
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
    pub(crate) fn next_segment(&self) -> ApiResult<u32> {
        const KEY: &str = "fabric/next_segment";
        let cur: u32 = self.store().get_meta(KEY).map_err(api)?.unwrap_or(1);
        self.store().put_meta(KEY, &(cur + 1)).map_err(api)?;
        Ok(cur)
    }

    /// Make sure a network is realised; returns it when Ready.
    pub(crate) async fn ready_network(&self, id: &Id) -> ApiResult<Network> {
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
