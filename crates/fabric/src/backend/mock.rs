// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! In-memory backend. Records every call so tests can assert on the
//! sequence, and keeps a picture of what "exists" so idempotency and
//! ordering bugs in the reconciler show up.

use super::{FipView, NetBackend, RouterView};
use crate::types::{Network, Port, PortBinding, SecurityGroup, Subnet};
use async_trait::async_trait;
use parking_lot::Mutex;
use pistack_types::{ApiError, ApiResult, Id};
use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;

/// One recorded backend call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Call {
    EnsureNetwork(Id),
    DeleteNetwork(Id),
    EnsureSubnet {
        network: Id,
        subnet: Id,
    },
    DeleteSubnet(Id),
    EnsurePort {
        network: Id,
        port: Id,
        addresses: Vec<IpAddr>,
        groups: Vec<Id>,
    },
    DeletePort(Id),
    EnsureRouter {
        router: Id,
        gateway: Option<Id>,
        interfaces: Vec<Id>,
    },
    DeleteRouter(Id),
    ApplySecurityGroup {
        sg: Id,
        rules: usize,
        members: Vec<Id>,
    },
    DeleteSecurityGroup(Id),
    AssociateFip {
        fip: Id,
        port: Option<Id>,
        fixed_ip: Option<IpAddr>,
    },
    DeleteFip(Id),
}

#[derive(Default)]
struct State {
    calls: Vec<Call>,
    networks: BTreeSet<Id>,
    subnets: BTreeSet<Id>,
    ports: BTreeMap<Id, Vec<IpAddr>>,
    routers: BTreeSet<Id>,
    groups: BTreeSet<Id>,
    fips: BTreeMap<Id, Option<Id>>,
    /// Operations that should fail, by call variant name. Consumed once.
    fail_once: BTreeSet<&'static str>,
}

/// See module docs.
#[derive(Default)]
pub struct MockBackend {
    state: Mutex<State>,
}

impl MockBackend {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every call so far, in order.
    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().calls.clone()
    }

    pub fn clear_calls(&self) {
        self.state.lock().calls.clear();
    }

    /// Make the next call of this variant (`"EnsurePort"`, ...) fail with
    /// `unavailable`, then succeed again. For testing error phases.
    pub fn fail_next(&self, variant: &'static str) {
        self.state.lock().fail_once.insert(variant);
    }

    pub fn has_network(&self, id: &Id) -> bool {
        self.state.lock().networks.contains(id)
    }
    pub fn has_subnet(&self, id: &Id) -> bool {
        self.state.lock().subnets.contains(id)
    }
    pub fn has_port(&self, id: &Id) -> bool {
        self.state.lock().ports.contains_key(id)
    }
    pub fn has_router(&self, id: &Id) -> bool {
        self.state.lock().routers.contains(id)
    }
    pub fn has_security_group(&self, id: &Id) -> bool {
        self.state.lock().groups.contains(id)
    }
    /// The port a floating IP is associated with, if the FIP is known.
    pub fn fip_target(&self, id: &Id) -> Option<Option<Id>> {
        self.state.lock().fips.get(id).cloned()
    }

    fn record(
        &self,
        variant: &'static str,
        call: Call,
        apply: impl FnOnce(&mut State),
    ) -> ApiResult<()> {
        let mut s = self.state.lock();
        s.calls.push(call);
        if s.fail_once.remove(variant) {
            return Err(ApiError::unavailable(format!(
                "mock backend: {variant} failed on request"
            )));
        }
        apply(&mut s);
        Ok(())
    }
}

#[async_trait]
impl NetBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }

    async fn ensure_network(&self, net: &Network) -> ApiResult<()> {
        let id = net.meta.id.clone();
        self.record("EnsureNetwork", Call::EnsureNetwork(id.clone()), |s| {
            s.networks.insert(id);
        })
    }

    async fn delete_network(&self, id: &Id) -> ApiResult<()> {
        let id = id.clone();
        self.record("DeleteNetwork", Call::DeleteNetwork(id.clone()), |s| {
            s.networks.remove(&id);
        })
    }

    async fn ensure_subnet(&self, net: &Network, subnet: &Subnet) -> ApiResult<()> {
        if !self.has_network(&net.meta.id) {
            return Err(ApiError::internal(format!(
                "mock: network {} not realised",
                net.meta.id
            )));
        }
        let id = subnet.meta.id.clone();
        self.record(
            "EnsureSubnet",
            Call::EnsureSubnet {
                network: net.meta.id.clone(),
                subnet: id.clone(),
            },
            |s| {
                s.subnets.insert(id);
            },
        )
    }

    async fn delete_subnet(&self, id: &Id) -> ApiResult<()> {
        let id = id.clone();
        self.record("DeleteSubnet", Call::DeleteSubnet(id.clone()), |s| {
            s.subnets.remove(&id);
        })
    }

    async fn ensure_port(
        &self,
        net: &Network,
        port: &Port,
        groups: &[SecurityGroup],
    ) -> ApiResult<PortBinding> {
        if !self.has_network(&net.meta.id) {
            return Err(ApiError::internal(format!(
                "mock: network {} not realised",
                net.meta.id
            )));
        }
        let id = port.meta.id.clone();
        let ips = port.spec.fixed_ips.clone();
        self.record(
            "EnsurePort",
            Call::EnsurePort {
                network: net.meta.id.clone(),
                port: id.clone(),
                addresses: ips.clone(),
                groups: groups.iter().map(|g| g.meta.id.clone()).collect(),
            },
            |s| {
                s.ports.insert(id, ips);
            },
        )?;
        let mut details = BTreeMap::new();
        details.insert("mac".into(), port.spec.mac.clone());
        details.insert("network".into(), net.meta.id.to_string());
        Ok(PortBinding {
            vif_type: "mock".into(),
            vif_details: details,
        })
    }

    async fn delete_port(&self, id: &Id) -> ApiResult<()> {
        let id = id.clone();
        self.record("DeletePort", Call::DeletePort(id.clone()), |s| {
            s.ports.remove(&id);
        })
    }

    async fn ensure_router(&self, view: &RouterView) -> ApiResult<()> {
        let id = view.router.meta.id.clone();
        self.record(
            "EnsureRouter",
            Call::EnsureRouter {
                router: id.clone(),
                gateway: view.gateway.as_ref().map(|(_, p)| p.meta.id.clone()),
                interfaces: view
                    .interfaces
                    .iter()
                    .map(|(s, _)| s.meta.id.clone())
                    .collect(),
            },
            |s| {
                s.routers.insert(id);
            },
        )
    }

    async fn delete_router(&self, id: &Id) -> ApiResult<()> {
        let id = id.clone();
        self.record("DeleteRouter", Call::DeleteRouter(id.clone()), |s| {
            s.routers.remove(&id);
        })
    }

    async fn apply_security_group(&self, sg: &SecurityGroup, members: &[Port]) -> ApiResult<()> {
        let id = sg.meta.id.clone();
        self.record(
            "ApplySecurityGroup",
            Call::ApplySecurityGroup {
                sg: id.clone(),
                rules: sg.spec.rules.len(),
                members: members.iter().map(|p| p.meta.id.clone()).collect(),
            },
            |s| {
                s.groups.insert(id);
            },
        )
    }

    async fn delete_security_group(&self, id: &Id) -> ApiResult<()> {
        let id = id.clone();
        self.record(
            "DeleteSecurityGroup",
            Call::DeleteSecurityGroup(id.clone()),
            |s| {
                s.groups.remove(&id);
            },
        )
    }

    async fn associate_fip(&self, view: &FipView) -> ApiResult<()> {
        let id = view.fip.meta.id.clone();
        let port = view.target.as_ref().map(|(p, _)| p.meta.id.clone());
        self.record(
            "AssociateFip",
            Call::AssociateFip {
                fip: id.clone(),
                port: port.clone(),
                fixed_ip: view.target.as_ref().map(|(_, ip)| *ip),
            },
            |s| {
                s.fips.insert(id, port);
            },
        )
    }

    async fn delete_fip(&self, id: &Id) -> ApiResult<()> {
        let id = id.clone();
        self.record("DeleteFip", Call::DeleteFip(id.clone()), |s| {
            s.fips.remove(&id);
        })
    }
}
