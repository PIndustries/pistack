// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The Service controller: a cluster IP and the endpoints behind it.
//!
//! Cluster IPs come from a CIDR (default `10.96.0.0/12`) through a counter
//! kept in the store's meta table, so they survive restarts and never
//! repeat while the counter lasts. Endpoints are the `Running` pods in the
//! service's project that are ready (see `PodStatus::is_ready`; a pod
//! without a readiness probe is ready once its containers run) and whose
//! labels match `spec.selector`. `NodePort` and
//! `LoadBalancer` services also hold one node port per service port, the
//! one `spec` asks for or the lowest free one in 30000-32767, and two
//! services never share one. Every change is handed to the
//! [`ServiceRealizer`].
//!
//! Listeners live in the process that programmed them, so the controller
//! remembers what it handed the realizer itself. A server that becomes
//! leader, or restarts, realizes every service once even when the stored
//! status already matches, and a failed realize is retried on the next
//! pass.

use super::{delete_quietly, patch_status, Controller};
use crate::realizer::ServiceRealizer;
use crate::selector::labels_match;
use crate::types::{
    delete_requested, set_condition, Endpoint, Pod, PodPhase, Service, ServiceKind, NODE_PORT_RANGE,
};
use ipnet::IpNet;
use pistack_store::{Collection, Store, StoreError};
use pistack_types::Id;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Arc, Mutex};

/// Default service CIDR.
pub const DEFAULT_SERVICE_CIDR: &str = "10.96.0.0/12";
/// Store meta key holding the next host offset in the CIDR.
pub const IP_COUNTER_KEY: &str = "loom/service_ip_next";

/// First usable host address (as an integer) and the number of usable
/// hosts in `cidr`.
fn cidr_range(cidr: &IpNet) -> (u128, u128) {
    let (network, host_bits): (u128, u32) = match cidr {
        IpNet::V4(n) => (u32::from(n.network()) as u128, 32 - n.prefix_len() as u32),
        IpNet::V6(n) => (u128::from(n.network()), 128 - n.prefix_len() as u32),
    };
    match host_bits {
        0 => (network, 1),
        1 => (network, 2),
        b if b >= 127 => (network + 1, u128::MAX - 2),
        b => (network + 1, (1u128 << b) - 2),
    }
}

/// Node ports `svc` should hold, keyed by service port. `taken` maps each
/// node port another service holds to that service.
fn node_ports_for(svc: &Service, taken: &BTreeMap<u16, Id>) -> Result<BTreeMap<u16, u16>, String> {
    let mut out = BTreeMap::new();
    if svc.spec.kind == ServiceKind::ClusterIp {
        return Ok(out);
    }
    let mut used: BTreeSet<u16> = taken
        .iter()
        .filter(|(_, owner)| **owner != svc.meta.id)
        .map(|(port, _)| *port)
        .collect();
    for p in &svc.spec.ports {
        if let Some(want) = p.node_port {
            if let Some(owner) = taken.get(&want).filter(|o| **o != svc.meta.id) {
                return Err(format!(
                    "node port {want} is already held by service {owner}"
                ));
            }
            out.insert(p.port, want);
            used.insert(want);
        }
    }
    for p in &svc.spec.ports {
        if out.contains_key(&p.port) {
            continue;
        }
        if let Some(&had) = svc.status.node_ports.get(&p.port) {
            if NODE_PORT_RANGE.contains(&had) && used.insert(had) {
                out.insert(p.port, had);
            }
        }
    }
    for p in &svc.spec.ports {
        if out.contains_key(&p.port) {
            continue;
        }
        let Some(free) = NODE_PORT_RANGE.clone().find(|n| !used.contains(n)) else {
            return Err("no node port is free".into());
        };
        used.insert(free);
        out.insert(p.port, free);
    }
    Ok(out)
}

/// What one realize call was given: cluster IP, endpoints and node ports.
type Programmed = (String, Vec<Endpoint>, BTreeMap<u16, u16>);

pub struct ServiceController {
    store: Store,
    cidr: IpNet,
    realizer: Arc<dyn ServiceRealizer>,
    /// Per service, what this process last realized successfully.
    programmed: Mutex<HashMap<Id, Programmed>>,
}

impl ServiceController {
    pub fn new(store: Store, cidr: IpNet, realizer: Arc<dyn ServiceRealizer>) -> Self {
        Self {
            store,
            cidr,
            realizer,
            programmed: Mutex::new(HashMap::new()),
        }
    }
    fn services(&self) -> Collection<Service> {
        self.store.collection::<Service>()
    }
    fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }

    /// Next free address: skips the network address, the broadcast address
    /// and anything a service already holds.
    fn allocate_ip(&self, taken: &[IpAddr]) -> Result<IpAddr, StoreError> {
        let mut next: u128 = self.store.get_meta(IP_COUNTER_KEY)?.unwrap_or(0);
        let (base, hosts) = cidr_range(&self.cidr);
        let mut tried = 0u128;
        loop {
            if tried >= hosts {
                return Err(StoreError::Conflict(format!(
                    "service CIDR {} exhausted",
                    self.cidr
                )));
            }
            let offset = next % hosts;
            next += 1;
            tried += 1;
            let ip = match self.cidr {
                IpNet::V4(_) => IpAddr::V4(Ipv4Addr::from((base + offset) as u32)),
                IpNet::V6(_) => IpAddr::V6(Ipv6Addr::from(base + offset)),
            };
            if taken.contains(&ip) {
                continue;
            }
            self.store.put_meta(IP_COUNTER_KEY, &next)?;
            return Ok(ip);
        }
    }

    fn programmed_lock(&self) -> std::sync::MutexGuard<'_, HashMap<Id, Programmed>> {
        self.programmed
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    async fn teardown(&self, svc: &Service) -> anyhow::Result<()> {
        self.programmed_lock().remove(&svc.meta.id);
        if let Err(e) = self.realizer.remove(svc).await {
            tracing::warn!(svc = %svc.meta.id, error = %e, "loom: realizer remove failed; deleting anyway");
        }
        delete_quietly(&self.services(), &svc.meta.id)?;
        tracing::info!(svc = %svc.meta.id, name = %svc.meta.name, "loom: service deleted");
        Ok(())
    }

    async fn converge(
        &self,
        svc: &Service,
        pods: &[Pod],
        taken: &[IpAddr],
        taken_ports: &BTreeMap<u16, Id>,
    ) -> anyhow::Result<()> {
        let cluster_ip = match &svc.status.cluster_ip {
            Some(ip) => ip.clone(),
            None => match &svc.spec.cluster_ip {
                Some(want) => want.clone(),
                None => self.allocate_ip(taken)?.to_string(),
            },
        };
        let endpoints = endpoints_for(svc, pods);

        let (node_ports, port_err) = match node_ports_for(svc, taken_ports) {
            Ok(ports) => (ports, None),
            Err(why) => (BTreeMap::new(), Some(why)),
        };
        let want: Programmed = (cluster_ip.clone(), endpoints.clone(), node_ports.clone());
        let changed = svc.status.cluster_ip.as_deref() != Some(cluster_ip.as_str())
            || svc.status.endpoints != endpoints
            || svc.status.node_ports != node_ports
            || self.programmed_lock().get(&svc.meta.id) != Some(&want);
        let mut lb = svc.status.load_balancer.clone();
        let mut realized = true;
        let mut why = String::new();
        if changed {
            let mut with_ip = svc.clone();
            with_ip.status.cluster_ip = Some(cluster_ip.clone());
            with_ip.status.node_ports = node_ports.clone();
            match self.realizer.realize(&with_ip, &endpoints).await {
                Ok(addrs) => {
                    lb = addrs;
                    self.programmed_lock().insert(svc.meta.id.clone(), want);
                }
                Err(e) => {
                    self.programmed_lock().remove(&svc.meta.id);
                    realized = false;
                    why = e.message.clone();
                    tracing::warn!(svc = %svc.meta.id, error = %e, "loom: service realize failed");
                }
            }
        }
        if let Some(e) = port_err {
            tracing::warn!(svc = %svc.meta.id, error = %e, "loom: service node port refused");
            realized = false;
            why = e;
        }
        let summary = format!("{} endpoints", endpoints.len());
        patch_status(&self.services(), &svc.meta.id, |s| {
            s.cluster_ip = Some(cluster_ip.clone());
            s.endpoints = endpoints.clone();
            s.node_ports = node_ports.clone();
            s.load_balancer = lb.clone();
            set_condition(
                &mut s.conditions,
                "Realized",
                realized,
                if realized {
                    "Programmed"
                } else {
                    "RealizeFailed"
                },
                if realized { &summary } else { &why },
            );
        })?;
        Ok(())
    }
}

#[async_trait::async_trait]
impl Controller for ServiceController {
    fn name(&self) -> &'static str {
        "service"
    }
    fn watches(&self) -> &'static [&'static str] {
        &[crate::types::kinds::POD, crate::types::kinds::SERVICE]
    }
    async fn stop_leading(&self) {
        self.programmed_lock().clear();
        self.realizer.release().await;
    }
    async fn reconcile_once(&self) -> anyhow::Result<()> {
        let services = self.services().list(None)?;
        let pods = self.pods().list(None)?;
        let mut taken: Vec<IpAddr> = services
            .iter()
            .filter_map(|s| {
                s.status
                    .cluster_ip
                    .as_deref()
                    .or(s.spec.cluster_ip.as_deref())
            })
            .filter_map(|ip| ip.parse().ok())
            .collect();
        let mut taken_ports: BTreeMap<u16, Id> = BTreeMap::new();
        for svc in &services {
            for node_port in svc.status.node_ports.values() {
                taken_ports.insert(*node_port, svc.meta.id.clone());
            }
        }
        for svc in &services {
            let res = if delete_requested(svc) {
                self.teardown(svc).await
            } else {
                let r = self.converge(svc, &pods, &taken, &taken_ports).await;
                let now = self.services().try_get(&svc.meta.id).ok().flatten();
                if let Some(now) = &now {
                    taken_ports.retain(|_, owner| *owner != svc.meta.id);
                    for node_port in now.status.node_ports.values() {
                        taken_ports.insert(*node_port, svc.meta.id.clone());
                    }
                }
                if let Some(ip) = now.and_then(|s| s.status.cluster_ip) {
                    if let Ok(ip) = ip.parse() {
                        if !taken.contains(&ip) {
                            taken.push(ip);
                        }
                    }
                }
                r
            };
            if let Err(e) = res {
                tracing::warn!(svc = %svc.meta.id, error = %e, "loom: service reconcile failed");
            }
        }
        Ok(())
    }
}

/// The pods a service sends traffic to: running and ready pods in the
/// service's project whose labels match its selector, sorted by pod id.
/// A pod still starting, or held back by a failing readiness probe, is
/// left out until it turns ready.
fn endpoints_for(svc: &Service, pods: &[Pod]) -> Vec<Endpoint> {
    let mut endpoints: Vec<Endpoint> = pods
        .iter()
        .filter(|p| {
            p.meta.project == svc.meta.project
                && !delete_requested(p)
                && p.status.phase == PodPhase::Running
                && p.status.is_ready()
                && p.status.pod_ip.is_some()
                && labels_match(&svc.spec.selector, &p.meta.labels)
        })
        .map(|p| Endpoint {
            ip: p.status.pod_ip.clone().unwrap_or_default(),
            pod: p.meta.id.clone(),
            host: p.status.host.clone(),
        })
        .collect();
    endpoints.sort_by(|a, b| a.pod.cmp(&b.pod));
    endpoints
}

#[cfg(test)]
mod node_port_tests {
    use super::*;
    use crate::types::{ServicePort, ServiceSpec, ServiceStatus};
    use pistack_types::Meta;

    fn svc(kind: ServiceKind, ports: &[(u16, Option<u16>)]) -> Service {
        Service {
            meta: Meta::new(crate::kinds::SERVICE, "web", None),
            spec: ServiceSpec {
                ports: ports
                    .iter()
                    .map(|(port, node_port)| ServicePort {
                        port: *port,
                        node_port: *node_port,
                        ..ServicePort::default()
                    })
                    .collect(),
                kind,
                ..ServiceSpec::default()
            },
            status: ServiceStatus::default(),
        }
    }

    /// The router plan reads services through a narrow view in forge. Keep
    /// the two shapes in step.
    #[test]
    fn the_router_plan_reads_the_service_record() {
        use pistack_types::{Kind, Spec};
        assert_eq!(<forge::ServiceView as Spec>::KIND, <Service as Kind>::KIND);
        let mut s = svc(ServiceKind::ClusterIp, &[(80, None)]);
        s.spec.ports[0].target_port = Some(8080);
        s.spec.ports.push(ServicePort {
            protocol: crate::types::Protocol::Udp,
            port: 53,
            ..ServicePort::default()
        });
        s.status.cluster_ip = Some("10.96.0.10".into());
        s.status.endpoints = vec![Endpoint {
            ip: "10.90.0.5".into(),
            pod: Id::new(crate::kinds::POD),
            host: None,
        }];
        let view: forge::ServiceRecord =
            serde_json::from_value(serde_json::to_value(&s).unwrap()).unwrap();
        assert_eq!(view.status.cluster_ip.as_deref(), Some("10.96.0.10"));
        assert_eq!(view.status.endpoints[0].ip, "10.90.0.5");
        assert_eq!(view.spec.ports[0].protocol, "TCP");
        assert_eq!(view.spec.ports[0].target_port, Some(8080));
        assert_eq!(view.spec.ports[1].protocol, "UDP");
    }

    #[test]
    fn cluster_ip_holds_none() {
        let s = svc(ServiceKind::ClusterIp, &[(80, None)]);
        assert!(node_ports_for(&s, &BTreeMap::new()).unwrap().is_empty());
    }

    #[test]
    fn lowest_free_and_explicit() {
        let other = Id::new(crate::kinds::SERVICE);
        let taken = BTreeMap::from([(30000, other.clone()), (30002, other)]);
        let s = svc(
            ServiceKind::NodePort,
            &[(80, None), (443, Some(30001)), (8080, None)],
        );
        let got = node_ports_for(&s, &taken).unwrap();
        assert_eq!(
            got,
            BTreeMap::from([(80, 30003), (443, 30001), (8080, 30004)])
        );
    }

    #[test]
    fn explicit_conflict_is_an_error() {
        let other = Id::new(crate::kinds::SERVICE);
        let taken = BTreeMap::from([(31000, other)]);
        let s = svc(ServiceKind::LoadBalancer, &[(80, Some(31000))]);
        let err = node_ports_for(&s, &taken).unwrap_err();
        assert!(err.contains("31000"), "{err}");
    }

    #[test]
    fn keeps_its_port_across_passes() {
        let mut s = svc(ServiceKind::NodePort, &[(80, None)]);
        s.status.node_ports.insert(80, 32000);
        let taken = BTreeMap::from([(32000, s.meta.id.clone())]);
        let got = node_ports_for(&s, &taken).unwrap();
        assert_eq!(got, BTreeMap::from([(80, 32000)]));
    }

    #[test]
    fn gives_up_a_kept_port_another_service_now_holds() {
        let mut s = svc(ServiceKind::NodePort, &[(80, None)]);
        s.status.node_ports.insert(80, 30000);
        let taken = BTreeMap::from([(30000, Id::new(crate::kinds::SERVICE))]);
        let got = node_ports_for(&s, &taken).unwrap();
        assert_eq!(got, BTreeMap::from([(80, 30001)]));
    }
}

#[cfg(test)]
mod endpoint_tests {
    use super::*;
    use crate::types::{PodSpec, PodStatus, ServiceSpec, ServiceStatus, POD_READY};
    use pistack_types::Meta;

    fn svc() -> Service {
        Service {
            meta: Meta::new(crate::kinds::SERVICE, "web", None),
            spec: ServiceSpec {
                selector: BTreeMap::from([("app".to_string(), "web".to_string())]),
                ..ServiceSpec::default()
            },
            status: ServiceStatus::default(),
        }
    }

    fn pod(name: &str, ip: &str, ready: bool) -> Pod {
        let mut meta = Meta::new(crate::kinds::POD, name, None);
        meta.labels = BTreeMap::from([("app".to_string(), "web".to_string())]);
        let mut status = PodStatus {
            phase: PodPhase::Running,
            pod_ip: Some(ip.to_string()),
            ..PodStatus::default()
        };
        set_condition(&mut status.conditions, POD_READY, ready, "Test", "");
        Pod {
            meta,
            spec: PodSpec::default(),
            status,
        }
    }

    #[test]
    fn running_but_not_ready_pod_is_left_out_until_ready() {
        let s = svc();
        let mut pods = vec![pod("a", "10.0.0.2", true), pod("b", "10.0.0.3", false)];
        let ips: Vec<String> = endpoints_for(&s, &pods).into_iter().map(|e| e.ip).collect();
        assert_eq!(ips, vec!["10.0.0.2".to_string()]);

        set_condition(&mut pods[1].status.conditions, POD_READY, true, "Test", "");
        let mut ips: Vec<String> = endpoints_for(&s, &pods).into_iter().map(|e| e.ip).collect();
        ips.sort();
        assert_eq!(ips, vec!["10.0.0.2".to_string(), "10.0.0.3".to_string()]);
    }

    #[test]
    fn pod_with_no_ready_condition_is_left_out() {
        let s = svc();
        let mut p = pod("a", "10.0.0.2", true);
        p.status.conditions.clear();
        assert!(endpoints_for(&s, &[p]).is_empty());
    }
}
