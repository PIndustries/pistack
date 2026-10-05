// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `Client`: the one place the network model is written.
//!
//! The HTTP handlers, the reconciler and other modules (`forge`, `loom`)
//! all go through here. Every method validates, allocates (IPAM) and writes
//! `spec`; nothing here touches the backend or `status`. That is the
//! reconciler's job (`reconcile.rs`), which runs on the events these writes
//! emit. The exceptions are documented on `create_port_for_device`.

use crate::backend::NetBackend;
use crate::ipam;
use crate::types::*;
use ipnet::IpNet;
use pistack_store::{Collection, Store, StoreError};
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};
use std::collections::{BTreeSet, HashMap};
use std::net::IpAddr;
use std::sync::Arc;

/// Name of the cloud-wide template security group and of each project's
/// clone of it.
pub const DEFAULT_SG: &str = "default";

#[derive(Clone)]
pub struct Client {
    store: Store,
    backend: Arc<dyn NetBackend>,
    /// Last-seen shape of each port the reconciler realised, so a Deleted
    /// event (which carries only the id) can still refresh the subnet's
    /// allocation count and the port's security groups.
    pub(crate) port_index: Arc<parking_lot::Mutex<HashMap<Id, PortRef>>>,
}

/// What the reconciler remembers about a port after it has gone.
#[derive(Clone, Debug)]
pub(crate) struct PortRef {
    pub subnet: Option<Id>,
    pub groups: Vec<Id>,
}

fn store_err(e: StoreError) -> ApiError {
    e.into()
}

impl Client {
    pub fn new(store: Store, backend: Arc<dyn NetBackend>) -> Self {
        Self {
            store,
            backend,
            port_index: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        }
    }

    pub fn store(&self) -> &Store {
        &self.store
    }
    pub fn backend(&self) -> &Arc<dyn NetBackend> {
        &self.backend
    }

    pub fn networks(&self) -> Collection<Network> {
        self.store.collection()
    }
    pub fn subnets(&self) -> Collection<Subnet> {
        self.store.collection()
    }
    pub fn ports(&self) -> Collection<Port> {
        self.store.collection()
    }
    pub fn routers(&self) -> Collection<Router> {
        self.store.collection()
    }
    pub fn security_groups(&self) -> Collection<SecurityGroup> {
        self.store.collection()
    }
    pub fn floating_ips(&self) -> Collection<FloatingIp> {
        self.store.collection()
    }

    // ---- shared lookups ---------------------------------------------------

    /// Ports of a network, optionally those plugged into `device`.
    pub fn ports_on_network(&self, network: &Id) -> ApiResult<Vec<Port>> {
        Ok(self
            .ports()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| &p.spec.network == network)
            .collect())
    }

    pub fn ports_of_device(&self, device: &Id) -> ApiResult<Vec<Port>> {
        Ok(self
            .ports()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| p.spec.device.as_ref() == Some(device))
            .collect())
    }

    /// Subnets of a network in creation order.
    pub fn subnets_of_network(&self, network: &Id) -> ApiResult<Vec<Subnet>> {
        let mut v: Vec<Subnet> = self
            .subnets()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|s| &s.spec.network == network)
            .collect();
        v.sort_by(|a, b| {
            a.meta
                .created_at
                .cmp(&b.meta.created_at)
                .then(a.meta.id.cmp(&b.meta.id))
        });
        Ok(v)
    }

    /// Addresses held in a subnet: port fixed IPs and floating IPs that fall
    /// inside its CIDR.
    pub fn used_addresses(&self, subnet: &Subnet) -> ApiResult<BTreeSet<IpAddr>> {
        let cidr = subnet.spec.cidr;
        let mut used = BTreeSet::new();
        for p in self.ports_on_network(&subnet.spec.network)? {
            used.extend(
                p.spec
                    .fixed_ips
                    .iter()
                    .copied()
                    .filter(|ip| cidr.contains(ip)),
            );
        }
        for f in self.floating_ips().list(None).map_err(store_err)? {
            if f.spec.network == subnet.spec.network {
                if let Some(a) = f.spec.address {
                    if cidr.contains(&a) {
                        used.insert(a);
                    }
                }
            }
        }
        Ok(used)
    }

    fn macs_on_network(&self, network: &Id) -> ApiResult<BTreeSet<String>> {
        Ok(self
            .ports_on_network(network)?
            .into_iter()
            .map(|p| p.spec.mac)
            .collect())
    }

    // ---- networks ---------------------------------------------------------

    pub fn create_network(&self, mut meta: Meta, spec: NetworkSpec) -> ApiResult<Network> {
        meta.id = Id::new(kind::NETWORK);
        validate_network(&spec)?;
        let net = Network {
            meta,
            spec,
            status: NetworkStatus::default(),
        };
        self.networks().create(net).map_err(store_err)
    }

    pub fn update_network(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Network) -> ApiResult<()>,
    ) -> ApiResult<Network> {
        let cur = self.networks().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        validate_network(&next.spec)?;
        if next.spec.kind != cur.spec.kind {
            return Err(ApiError::bad_request("network kind is immutable"));
        }
        self.networks().update(next, expected).map_err(store_err)
    }

    pub fn delete_network(&self, id: &Id) -> ApiResult<Network> {
        let net = self.networks().get(id).map_err(store_err)?;
        if !self.subnets_of_network(id)?.is_empty() {
            return Err(ApiError::conflict(format!(
                "network {id} still has subnets"
            )));
        }
        if !self.ports_on_network(id)?.is_empty() {
            return Err(ApiError::conflict(format!("network {id} still has ports")));
        }
        if self
            .routers()
            .list(None)
            .map_err(store_err)?
            .iter()
            .any(|r| r.spec.external_network.as_ref() == Some(id))
        {
            return Err(ApiError::conflict(format!(
                "network {id} is a router gateway"
            )));
        }
        let _ = net;
        self.networks().delete(id).map_err(store_err)
    }

    // ---- subnets ----------------------------------------------------------

    pub fn create_subnet(&self, mut meta: Meta, spec: SubnetSpec) -> ApiResult<Subnet> {
        meta.id = Id::new(kind::SUBNET);
        let net = self.networks().get(&spec.network).map_err(store_err)?;
        check_same_project(&meta, &net.meta, "network")?;
        ipam::validate_subnet(&spec)?;
        for other in self.subnets_of_network(&spec.network)? {
            if overlaps(spec.cidr, other.spec.cidr) {
                return Err(ApiError::conflict(format!(
                    "cidr {} overlaps subnet {} ({})",
                    spec.cidr, other.meta.name, other.spec.cidr
                )));
            }
        }
        let sub = Subnet {
            meta,
            spec,
            status: SubnetStatus::default(),
        };
        self.subnets().create(sub).map_err(store_err)
    }

    pub fn update_subnet(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Subnet) -> ApiResult<()>,
    ) -> ApiResult<Subnet> {
        let cur = self.subnets().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.network != cur.spec.network || next.spec.cidr != cur.spec.cidr {
            return Err(ApiError::bad_request(
                "subnet network and cidr are immutable",
            ));
        }
        ipam::validate_subnet(&next.spec)?;
        // Shrinking pools or moving the gateway must not strand an address.
        if next.spec.gateway != cur.spec.gateway
            || next.spec.allocation_pools != cur.spec.allocation_pools
        {
            let used = self.used_addresses(&cur)?;
            for ip in &used {
                if ipam::is_reserved(&next.spec, *ip) {
                    return Err(ApiError::conflict(format!(
                        "{ip} is in use and would become reserved"
                    )));
                }
            }
        }
        self.subnets().update(next, expected).map_err(store_err)
    }

    pub fn delete_subnet(&self, id: &Id) -> ApiResult<Subnet> {
        let sub = self.subnets().get(id).map_err(store_err)?;
        for p in self.ports_on_network(&sub.spec.network)? {
            if p.spec.subnet.as_ref() == Some(id)
                || p.spec.fixed_ips.iter().any(|ip| sub.spec.cidr.contains(ip))
            {
                let what = if p.is_router_port() {
                    "a router interface"
                } else {
                    "ports"
                };
                return Err(ApiError::conflict(format!("subnet {id} still has {what}")));
            }
        }
        self.subnets().delete(id).map_err(store_err)
    }

    // ---- ports ------------------------------------------------------------

    /// Create a port. Fills in subnet, fixed IPs, MAC and the project's
    /// default security group when the spec leaves them empty.
    pub fn create_port(&self, mut meta: Meta, mut spec: PortSpec) -> ApiResult<Port> {
        meta.id = Id::new(kind::PORT);
        let net = self.networks().get(&spec.network).map_err(store_err)?;
        check_same_project(&meta, &net.meta, "network")?;

        // Subnet: explicit, or the network's first.
        let subnets = self.subnets_of_network(&spec.network)?;
        let subnet = match &spec.subnet {
            Some(sid) => Some(
                subnets
                    .iter()
                    .find(|s| &s.meta.id == sid)
                    .cloned()
                    .ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "subnet {sid} is not on network {}",
                            spec.network
                        ))
                    })?,
            ),
            None => subnets.first().cloned(),
        };
        spec.subnet = subnet.as_ref().map(|s| s.meta.id.clone());

        // Addresses.
        match &subnet {
            Some(sub) => {
                let used = self.used_addresses(sub)?;
                if spec.fixed_ips.is_empty() {
                    spec.fixed_ips.push(ipam::allocate(&sub.spec, &used)?);
                } else {
                    // A router's interface port legitimately holds the gateway.
                    self.validate_fixed_ips(
                        &subnets,
                        &spec.fixed_ips,
                        is_router_device(&spec.device),
                    )?;
                }
            }
            None if !spec.fixed_ips.is_empty() => {
                return Err(ApiError::bad_request(
                    "network has no subnet; fixed_ips must be empty",
                ));
            }
            None => {}
        }

        // MAC.
        let macs = self.macs_on_network(&spec.network)?;
        if spec.mac.is_empty() {
            spec.mac = ipam::generate_mac(&meta.id.uuid, &macs)?;
        } else {
            spec.mac = ipam::format_mac(ipam::parse_mac(&spec.mac)?);
            if macs.contains(&spec.mac) {
                return Err(ApiError::conflict(format!(
                    "mac {} is already in use on this network",
                    spec.mac
                )));
            }
        }

        // Security groups.
        if spec.port_security && spec.security_groups.is_empty() && !is_router_device(&spec.device)
        {
            if let Some(p) = &meta.project {
                spec.security_groups
                    .push(self.default_security_group(p)?.meta.id);
            }
        }
        if !spec.port_security {
            spec.security_groups.clear();
        }
        self.validate_security_groups(&meta, &spec.security_groups)?;

        // Gateway and interface ports belong to the router, not the tenant quota.
        let counted = !is_router_device(&spec.device);
        if counted {
            crate::usage::admit_port(&self.store, meta.project.as_ref())?;
        }
        let port = Port {
            meta,
            spec,
            status: PortStatus::default(),
        };
        let port = self.ports().create(port).map_err(store_err)?;
        if counted {
            crate::usage::note_port_create(&self.store, port.meta.project.as_ref());
        }
        Ok(port)
    }

    pub fn update_port(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Port) -> ApiResult<()>,
    ) -> ApiResult<Port> {
        let cur = self.ports().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.network != cur.spec.network || next.spec.subnet != cur.spec.subnet {
            return Err(ApiError::bad_request(
                "port network and subnet are immutable",
            ));
        }
        if next.spec.mac != cur.spec.mac {
            next.spec.mac = ipam::format_mac(ipam::parse_mac(&next.spec.mac)?);
            if next.spec.mac != cur.spec.mac
                && self
                    .macs_on_network(&cur.spec.network)?
                    .contains(&next.spec.mac)
            {
                return Err(ApiError::conflict(format!(
                    "mac {} is already in use on this network",
                    next.spec.mac
                )));
            }
        }
        if next.spec.fixed_ips != cur.spec.fixed_ips {
            let subnets = self.subnets_of_network(&cur.spec.network)?;
            let added: Vec<IpAddr> = next
                .spec
                .fixed_ips
                .iter()
                .copied()
                .filter(|ip| !cur.spec.fixed_ips.contains(ip))
                .collect();
            self.validate_fixed_ips(&subnets, &added, cur.is_router_port())?;
        }
        if !next.spec.port_security {
            next.spec.security_groups.clear();
        }
        self.validate_security_groups(&next.meta, &next.spec.security_groups)?;
        self.ports().update(next, expected).map_err(store_err)
    }

    /// Delete a user port. Floating IPs pointing at it are disassociated.
    /// Router ports are refused; they go with their router.
    pub fn delete_port(&self, id: &Id) -> ApiResult<Port> {
        let port = self.ports().get(id).map_err(store_err)?;
        if port.is_router_port() {
            return Err(ApiError::conflict(format!(
                "port {id} belongs to a router; detach it there"
            )));
        }
        self.delete_port_unchecked(&port)
    }

    fn delete_port_unchecked(&self, port: &Port) -> ApiResult<Port> {
        for f in self.floating_ips().list(None).map_err(store_err)? {
            if f.spec.port.as_ref() == Some(&port.meta.id) {
                self.floating_ips()
                    .modify(&f.meta.id, |x| {
                        x.spec.port = None;
                        Ok(())
                    })
                    .map_err(store_err)?;
            }
        }
        let counted = !port.is_router_port();
        let port = self.ports().delete(&port.meta.id).map_err(store_err)?;
        if counted {
            crate::usage::note_port_delete(&self.store, port.meta.project.as_ref());
        }
        Ok(port)
    }

    fn validate_fixed_ips(
        &self,
        subnets: &[Subnet],
        ips: &[IpAddr],
        allow_gateway: bool,
    ) -> ApiResult<()> {
        let mut seen = BTreeSet::new();
        for ip in ips {
            if !seen.insert(*ip) {
                return Err(ApiError::bad_request(format!("duplicate fixed ip {ip}")));
            }
            let sub = subnets
                .iter()
                .find(|s| s.spec.cidr.contains(ip))
                .ok_or_else(|| {
                    ApiError::bad_request(format!("{ip} is not in any subnet of this network"))
                })?;
            let used = self.used_addresses(sub)?;
            if allow_gateway && sub.spec.gateway == Some(*ip) {
                if used.contains(ip) {
                    return Err(ApiError::conflict(format!("{ip} is already in use")));
                }
                continue;
            }
            ipam::validate_fixed(&sub.spec, *ip, &used)?;
        }
        Ok(())
    }

    fn validate_security_groups(&self, meta: &Meta, groups: &[Id]) -> ApiResult<()> {
        for g in groups {
            let sg = self.security_groups().get(g).map_err(store_err)?;
            check_same_project(meta, &sg.meta, "security group")?;
        }
        Ok(())
    }

    // ---- routers ----------------------------------------------------------

    pub fn create_router(&self, mut meta: Meta, spec: RouterSpec) -> ApiResult<Router> {
        meta.id = Id::new(kind::ROUTER);
        validate_routes(&spec.routes)?;
        let ext = match &spec.external_network {
            Some(n) => Some(self.external_network(n)?),
            None => None,
        };
        let interfaces = spec.interfaces.clone();
        let spec = RouterSpec {
            interfaces: Vec::new(),
            ..spec
        };
        let router = self
            .routers()
            .create(Router {
                meta,
                spec,
                status: RouterStatus::default(),
            })
            .map_err(store_err)?;
        let plumb = (|| -> ApiResult<Router> {
            if let Some(net) = &ext {
                self.create_gateway_port(&router, net)?;
            }
            if interfaces.is_empty() {
                Ok(router.clone())
            } else {
                self.set_router_interfaces(&router.meta.id, None, &interfaces)
            }
        })();
        match plumb {
            Ok(r) => Ok(r),
            Err(e) => {
                // Roll back so a failed create leaves nothing behind.
                let _ = self.delete_router(&router.meta.id);
                Err(e)
            }
        }
    }

    /// Update routes and the external gateway. `interfaces` changes are
    /// ignored here; use `set_router_interfaces`.
    pub fn update_router(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Router) -> ApiResult<()>,
    ) -> ApiResult<Router> {
        let cur = self.routers().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        next.spec.interfaces = cur.spec.interfaces.clone();
        validate_routes(&next.spec.routes)?;
        if next.spec.external_network != cur.spec.external_network {
            if let Some(old) = &cur.spec.external_network {
                for p in self.ports_of_device(id)? {
                    if &p.spec.network == old {
                        self.delete_port_unchecked(&p)?;
                    }
                }
            }
            if let Some(n) = &next.spec.external_network {
                let net = self.external_network(n)?;
                self.create_gateway_port(&cur, &net)?;
            }
        }
        self.routers().update(next, expected).map_err(store_err)
    }

    /// Replace the router's interface set. Adds a port holding each new
    /// subnet's gateway address, removes ports for subnets dropped.
    pub fn set_router_interfaces(
        &self,
        id: &Id,
        expected: Option<u64>,
        subnets: &[Id],
    ) -> ApiResult<Router> {
        let router = self.routers().get(id).map_err(store_err)?;
        if let Some(exp) = expected {
            if exp != router.meta.revision {
                return Err(ApiError::conflict(format!(
                    "resource changed: expected revision {exp}, found {}",
                    router.meta.revision
                )));
            }
        }
        let want: BTreeSet<Id> = subnets.iter().cloned().collect();
        let have: BTreeSet<Id> = router.spec.interfaces.iter().cloned().collect();
        let ports = self.ports_of_device(id)?;

        for sid in want.difference(&have) {
            let sub = self.subnets().get(sid).map_err(store_err)?;
            check_same_project(&router.meta, &sub.meta, "subnet")?;
            let gw = sub.spec.gateway.ok_or_else(|| {
                ApiError::bad_request(format!(
                    "subnet {sid} has no gateway; cannot attach a router"
                ))
            })?;
            // One router per subnet: its gateway address can only be in one place.
            for other in self.routers().list(None).map_err(store_err)? {
                if other.meta.id != router.meta.id && other.spec.interfaces.contains(sid) {
                    return Err(ApiError::conflict(format!(
                        "subnet {sid} is already attached to router {}",
                        other.meta.id
                    )));
                }
            }
            let mut meta = Meta::new(
                kind::PORT,
                format!("{}-{}", router.meta.id, sid),
                router.meta.project.clone(),
            );
            meta.description = format!(
                "router {} interface on subnet {}",
                router.meta.name, sub.meta.name
            );
            let spec = PortSpec {
                network: sub.spec.network.clone(),
                subnet: Some(sid.clone()),
                fixed_ips: vec![gw],
                mac: String::new(),
                device: Some(router.meta.id.clone()),
                security_groups: Vec::new(),
                port_security: false,
            };
            self.create_port_as_router(meta, spec)?;
        }
        for sid in have.difference(&want) {
            for p in ports.iter().filter(|p| p.spec.subnet.as_ref() == Some(sid)) {
                self.delete_port_unchecked(p)?;
            }
        }
        let mut ordered: Vec<Id> = Vec::new();
        for s in subnets {
            if !ordered.contains(s) {
                ordered.push(s.clone());
            }
        }
        self.routers()
            .modify(id, |r| {
                r.spec.interfaces = ordered.clone();
                Ok(())
            })
            .map_err(store_err)
    }

    pub fn delete_router(&self, id: &Id) -> ApiResult<Router> {
        let router = self.routers().get(id).map_err(store_err)?;
        for f in self.floating_ips().list(None).map_err(store_err)? {
            if f.status.router.as_ref() == Some(id) && f.spec.port.is_some() {
                self.floating_ips()
                    .modify(&f.meta.id, |x| {
                        x.spec.port = None;
                        Ok(())
                    })
                    .map_err(store_err)?;
            }
        }
        for p in self.ports_of_device(id)? {
            self.delete_port_unchecked(&p)?;
        }
        let _ = router;
        self.routers().delete(id).map_err(store_err)
    }

    fn external_network(&self, id: &Id) -> ApiResult<Network> {
        let net = self.networks().get(id).map_err(store_err)?;
        if !net.spec.external {
            return Err(ApiError::bad_request(format!(
                "network {id} is not external"
            )));
        }
        Ok(net)
    }

    fn create_gateway_port(&self, router: &Router, net: &Network) -> ApiResult<Port> {
        if self.subnets_of_network(&net.meta.id)?.is_empty() {
            return Err(ApiError::bad_request(format!(
                "external network {} has no subnet",
                net.meta.id
            )));
        }
        let mut meta = Meta::new(
            kind::PORT,
            format!("{}-gw", router.meta.id),
            router.meta.project.clone(),
        );
        meta.description = format!("router {} gateway on {}", router.meta.name, net.meta.name);
        let spec = PortSpec {
            network: net.meta.id.clone(),
            subnet: None,
            fixed_ips: Vec::new(),
            mac: String::new(),
            device: Some(router.meta.id.clone()),
            security_groups: Vec::new(),
            port_security: false,
        };
        self.create_port_as_router(meta, spec)
    }

    /// Router ports live in the router's project; `create_port` already
    /// allows a project port on a cloud-wide (external) network.
    fn create_port_as_router(&self, meta: Meta, spec: PortSpec) -> ApiResult<Port> {
        self.create_port(meta, spec)
    }

    // ---- security groups --------------------------------------------------

    pub fn create_security_group(
        &self,
        mut meta: Meta,
        spec: SecurityGroupSpec,
    ) -> ApiResult<SecurityGroup> {
        meta.id = Id::new(kind::SECGROUP);
        self.validate_rules(&meta, &spec.rules, Some(&meta.id))?;
        self.security_groups()
            .create(SecurityGroup {
                meta,
                spec,
                status: SecurityGroupStatus::default(),
            })
            .map_err(store_err)
    }

    pub fn update_security_group(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut SecurityGroup) -> ApiResult<()>,
    ) -> ApiResult<SecurityGroup> {
        let cur = self.security_groups().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        self.validate_rules(&next.meta, &next.spec.rules, Some(id))?;
        self.security_groups()
            .update(next, expected)
            .map_err(store_err)
    }

    pub fn add_rule(&self, id: &Id, rule: Rule) -> ApiResult<SecurityGroup> {
        self.update_security_group(id, None, |sg| {
            if sg.spec.rules.iter().any(|r| r.id == rule.id) {
                return Err(ApiError::conflict(format!(
                    "rule {} already exists",
                    rule.id
                )));
            }
            sg.spec.rules.push(rule);
            Ok(())
        })
    }

    pub fn remove_rule(&self, id: &Id, rule: uuid::Uuid) -> ApiResult<SecurityGroup> {
        self.update_security_group(id, None, |sg| {
            let before = sg.spec.rules.len();
            sg.spec.rules.retain(|r| r.id != rule);
            if sg.spec.rules.len() == before {
                return Err(ApiError::not_found(format!("rule {rule} not found")));
            }
            Ok(())
        })
    }

    pub fn delete_security_group(&self, id: &Id) -> ApiResult<SecurityGroup> {
        let sg = self.security_groups().get(id).map_err(store_err)?;
        if sg.meta.project.is_none() && sg.meta.name == DEFAULT_SG {
            return Err(ApiError::forbidden(
                "the cloud-wide default security group template cannot be deleted",
            ));
        }
        let users = self
            .ports()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|p| p.spec.security_groups.contains(id))
            .count();
        if users > 0 {
            return Err(ApiError::conflict(format!(
                "security group {id} is used by {users} port(s)"
            )));
        }
        for other in self.security_groups().list(None).map_err(store_err)? {
            if other.meta.id != *id
                && other
                    .spec
                    .rules
                    .iter()
                    .any(|r| r.remote == Remote::Group(id.clone()))
            {
                return Err(ApiError::conflict(format!(
                    "security group {id} is referenced by rules of {}",
                    other.meta.id
                )));
            }
        }
        self.security_groups().delete(id).map_err(store_err)
    }

    fn validate_rules(&self, meta: &Meta, rules: &[Rule], self_id: Option<&Id>) -> ApiResult<()> {
        let mut ids = BTreeSet::new();
        for r in rules {
            if !ids.insert(r.id) {
                return Err(ApiError::bad_request(format!("duplicate rule id {}", r.id)));
            }
            if let Some(pr) = r.port_range {
                if pr.min > pr.max {
                    return Err(ApiError::bad_request(format!(
                        "port range {}-{} is inverted",
                        pr.min, pr.max
                    )));
                }
                if !matches!(r.protocol, Some(Protocol::Tcp) | Some(Protocol::Udp)) {
                    return Err(ApiError::bad_request(
                        "port_range requires protocol tcp or udp",
                    ));
                }
            }
            match &r.remote {
                Remote::Any => {}
                Remote::Cidr(c) => {
                    let v4 = matches!(c, IpNet::V4(_));
                    if v4 != (r.ethertype == EtherType::IPv4) {
                        return Err(ApiError::bad_request(format!(
                            "remote cidr {c} does not match ethertype"
                        )));
                    }
                }
                Remote::Group(g) => {
                    if Some(g) != self_id {
                        let other = self.security_groups().get(g).map_err(store_err)?;
                        check_same_project(meta, &other.meta, "remote security group")?;
                    }
                }
            }
        }
        Ok(())
    }

    /// Create the cloud-wide template "default" group if it is missing.
    /// Allow all egress; allow ingress from members of the same group.
    pub fn ensure_default_template(&self) -> ApiResult<SecurityGroup> {
        match self.security_groups().get_by_name(None, DEFAULT_SG) {
            Ok(sg) => return Ok(sg),
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(store_err(e)),
        }
        let mut meta = Meta::new(kind::SECGROUP, DEFAULT_SG, None);
        meta.description = "Template for each project's default security group".into();
        let me = meta.id.clone();
        let spec = SecurityGroupSpec {
            rules: default_rules(&me),
        };
        match self.security_groups().create(SecurityGroup {
            meta,
            spec,
            status: SecurityGroupStatus::default(),
        }) {
            Ok(sg) => Ok(sg),
            // Lost a race with another writer; theirs wins.
            Err(StoreError::Conflict(_)) => self
                .security_groups()
                .get_by_name(None, DEFAULT_SG)
                .map_err(store_err),
            Err(e) => Err(store_err(e)),
        }
    }

    /// The project's "default" security group, cloned from the template on
    /// first use.
    pub fn default_security_group(&self, project: &Id) -> ApiResult<SecurityGroup> {
        match self
            .security_groups()
            .get_by_name(Some(project), DEFAULT_SG)
        {
            Ok(sg) => return Ok(sg),
            Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(store_err(e)),
        }
        let template = self.ensure_default_template()?;
        let mut meta = Meta::new(kind::SECGROUP, DEFAULT_SG, Some(project.clone()));
        meta.description = "Default security group".into();
        meta.labels = template.meta.labels.clone();
        let me = meta.id.clone();
        let rules = template
            .spec
            .rules
            .iter()
            .map(|r| Rule {
                id: uuid::Uuid::now_v7(),
                remote: match &r.remote {
                    Remote::Group(g) if *g == template.meta.id => Remote::Group(me.clone()),
                    other => other.clone(),
                },
                ..r.clone()
            })
            .collect();
        match self.security_groups().create(SecurityGroup {
            meta,
            spec: SecurityGroupSpec { rules },
            status: SecurityGroupStatus::default(),
        }) {
            Ok(sg) => Ok(sg),
            Err(StoreError::Conflict(_)) => self
                .security_groups()
                .get_by_name(Some(project), DEFAULT_SG)
                .map_err(store_err),
            Err(e) => Err(store_err(e)),
        }
    }

    // ---- floating ips -----------------------------------------------------

    pub fn create_floating_ip(
        &self,
        mut meta: Meta,
        mut spec: FloatingIpSpec,
    ) -> ApiResult<FloatingIp> {
        meta.id = Id::new(kind::FLOATING_IP);
        let net = self.external_network(&spec.network)?;
        let subnets = self.subnets_of_network(&net.meta.id)?;
        if subnets.is_empty() {
            return Err(ApiError::bad_request(format!(
                "external network {} has no subnet",
                net.meta.id
            )));
        }
        match spec.address {
            Some(a) => self.validate_fixed_ips(&subnets, std::slice::from_ref(&a), false)?,
            None => {
                let mut picked = None;
                for sub in &subnets {
                    let used = self.used_addresses(sub)?;
                    if let Ok(ip) = ipam::allocate(&sub.spec, &used) {
                        picked = Some(ip);
                        break;
                    }
                }
                spec.address = Some(picked.ok_or_else(|| {
                    ApiError::conflict("no free floating ips")
                        .with_detail("network", net.meta.id.to_string())
                })?);
            }
        }
        if let Some(p) = &spec.port {
            self.validate_fip_target(&meta, &meta.id, p)?;
        }
        crate::usage::admit_floating_ip(&self.store, meta.project.as_ref())?;
        let fip = self
            .floating_ips()
            .create(FloatingIp {
                meta,
                spec,
                status: FloatingIpStatus::default(),
            })
            .map_err(store_err)?;
        crate::usage::note_floating_ip_create(&self.store, fip.meta.project.as_ref());
        Ok(fip)
    }

    /// Point the floating IP at a machine's current IPv4.
    ///
    /// A port is created on the longest-prefix subnet that contains that
    /// address, in this floating IP's project. A later call reuses that port.
    pub fn associate_to_machine(&self, id: &Id, machine: &Id) -> ApiResult<FloatingIp> {
        let fip = self.floating_ips().get(id).map_err(store_err)?;
        let guest = crate::machine::guest_ipv4(&self.store, machine)?;
        check_same_project(&fip.meta, &guest.meta, "machine")?;
        let port = self.ensure_machine_port(&fip.meta, machine, guest.ipv4, &guest.meta.name)?;
        self.associate_floating_ip(id, Some(port))
    }

    fn ensure_machine_port(
        &self,
        fip: &Meta,
        machine: &Id,
        addr: IpAddr,
        machine_name: &str,
    ) -> ApiResult<Id> {
        let ports = self.ports().list(None).map_err(store_err)?;
        if let Some(existing) = ports
            .iter()
            .find(|p| p.meta.project == fip.project && p.spec.fixed_ips.contains(&addr))
        {
            if existing.is_router_port() {
                return Err(ApiError::conflict(format!(
                    "{addr} is held by router port {}",
                    existing.meta.id
                )));
            }
            if existing.spec.device.as_ref().is_some_and(|d| d != machine) {
                return Err(ApiError::conflict(format!(
                    "{addr} is held by port {} on another machine",
                    existing.meta.id
                )));
            }
            return Ok(existing.meta.id.clone());
        }

        let mut subnets: Vec<Subnet> = self
            .subnets()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|s| {
                s.spec.cidr.contains(&addr) && check_same_project(fip, &s.meta, "subnet").is_ok()
            })
            .collect();
        subnets.sort_by(|a, b| {
            prefix_len(b.spec.cidr)
                .cmp(&prefix_len(a.spec.cidr))
                .then(a.meta.id.cmp(&b.meta.id))
        });
        let Some(sub) = subnets.into_iter().next() else {
            return Err(ApiError::unprocessable(format!(
                "no subnet contains {addr}"
            )));
        };
        let name = if machine_name.is_empty() {
            machine.to_string()
        } else {
            format!("{machine_name}-{addr}")
        };
        let port = self.create_port(
            Meta::new(kind::PORT, name, fip.project.clone()),
            PortSpec {
                network: sub.spec.network.clone(),
                subnet: Some(sub.meta.id.clone()),
                fixed_ips: vec![addr],
                device: Some(machine.clone()),
                ..PortSpec::default()
            },
        )?;
        Ok(port.meta.id)
    }

    /// Point the floating IP at `port`, or detach it with `None`.
    pub fn associate_floating_ip(&self, id: &Id, port: Option<Id>) -> ApiResult<FloatingIp> {
        let fip = self.floating_ips().get(id).map_err(store_err)?;
        if let Some(p) = &port {
            self.validate_fip_target(&fip.meta, id, p)?;
        }
        self.floating_ips()
            .modify(id, |f| {
                f.spec.port = port.clone();
                Ok(())
            })
            .map_err(store_err)
    }

    pub fn update_floating_ip(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut FloatingIp) -> ApiResult<()>,
    ) -> ApiResult<FloatingIp> {
        let cur = self.floating_ips().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.status = cur.status.clone();
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        if next.spec.network != cur.spec.network || next.spec.address != cur.spec.address {
            return Err(ApiError::bad_request(
                "floating ip network and address are immutable",
            ));
        }
        if next.spec.port != cur.spec.port {
            if let Some(p) = &next.spec.port {
                self.validate_fip_target(&next.meta, id, p)?;
            }
        }
        self.floating_ips()
            .update(next, expected)
            .map_err(store_err)
    }

    pub fn delete_floating_ip(&self, id: &Id) -> ApiResult<FloatingIp> {
        let fip = self.floating_ips().delete(id).map_err(store_err)?;
        crate::usage::note_floating_ip_delete(&self.store, fip.meta.project.as_ref());
        Ok(fip)
    }

    fn validate_fip_target(&self, meta: &Meta, fip: &Id, port: &Id) -> ApiResult<()> {
        let p = self.ports().get(port).map_err(store_err)?;
        check_same_project(meta, &p.meta, "port")?;
        if p.is_router_port() {
            return Err(ApiError::bad_request(format!(
                "port {port} belongs to a router"
            )));
        }
        if p.spec.fixed_ips.is_empty() {
            return Err(ApiError::bad_request(format!(
                "port {port} has no fixed ip"
            )));
        }
        for other in self.floating_ips().list(None).map_err(store_err)? {
            if other.meta.id != *fip && other.spec.port.as_ref() == Some(port) {
                return Err(ApiError::conflict(format!(
                    "port {port} already has floating ip {}",
                    other.meta.id
                )));
            }
        }
        Ok(())
    }

    // ---- for forge / loom -------------------------------------------------

    /// Create a port on `network` (name or id, looked up in `project` then
    /// cloud-wide) plugged into `device`, and realise it on the backend
    /// before returning so the caller gets a binding. The reconciler's own
    /// pass sees the status already current and skips it.
    pub async fn create_port_for_device(
        &self,
        project: &Id,
        network: &str,
        device: Id,
    ) -> ApiResult<Port> {
        self.create_port_for_device_with(project, network, device, &[])
            .await
    }

    /// Like [`Self::create_port_for_device`]. `groups` replaces the default
    /// security group when it is not empty.
    pub async fn create_port_for_device_with(
        &self,
        project: &Id,
        network: &str,
        device: Id,
        groups: &[Id],
    ) -> ApiResult<Port> {
        let net = match self.networks().resolve(Some(project), network) {
            Ok(n) => n,
            Err(StoreError::NotFound(_)) => {
                self.networks().resolve(None, network).map_err(store_err)?
            }
            Err(e) => return Err(store_err(e)),
        };
        let idx = self.ports_of_device(&device)?.len();
        let mut meta = Meta::new(
            kind::PORT,
            format!("{device}-eth{idx}"),
            Some(project.clone()),
        );
        meta.labels
            .insert("pistack.io/device".into(), device.to_string());
        let mut spec = PortSpec {
            network: net.meta.id.clone(),
            device: Some(device),
            ..PortSpec::default()
        };
        if !groups.is_empty() {
            spec.security_groups = groups.to_vec();
        }
        let port = self.create_port(meta, spec)?;
        self.reconcile_port(&port.meta.id).await;
        self.ports().get(&port.meta.id).map_err(store_err)
    }

    /// Delete every port plugged into `device`. Floating IPs are detached.
    pub fn release_device_ports(&self, device: &Id) -> ApiResult<Vec<Port>> {
        let mut out = Vec::new();
        for p in self.ports_of_device(device)? {
            out.push(self.delete_port_unchecked(&p)?);
        }
        Ok(out)
    }
}

/// Rules of the template "default" group: all egress out, ingress only from
/// the group itself. `me` is the template's own id.
pub fn default_rules(me: &Id) -> Vec<Rule> {
    vec![
        Rule::new(Direction::Egress, EtherType::IPv4),
        Rule::new(Direction::Egress, EtherType::IPv6),
        Rule::new(Direction::Ingress, EtherType::IPv4).remote(Remote::Group(me.clone())),
        Rule::new(Direction::Ingress, EtherType::IPv6).remote(Remote::Group(me.clone())),
    ]
}

fn validate_network(spec: &NetworkSpec) -> ApiResult<()> {
    if !(68..=9216).contains(&spec.mtu) {
        return Err(ApiError::bad_request(format!(
            "mtu {} out of range 68-9216",
            spec.mtu
        )));
    }
    if let NetworkKind::Provider { vlan, physnet } = &spec.kind {
        if let Some(v) = vlan {
            if !(1..=4094).contains(v) {
                return Err(ApiError::bad_request(format!(
                    "vlan {v} out of range 1-4094"
                )));
            }
        }
        if physnet.is_empty() {
            return Err(ApiError::bad_request("provider network needs a physnet"));
        }
    }
    Ok(())
}

fn validate_routes(routes: &[StaticRoute]) -> ApiResult<()> {
    for r in routes {
        if r.destination.addr().is_ipv4() != r.nexthop.is_ipv4() {
            return Err(ApiError::bad_request(format!(
                "route {} via {}: address family mismatch",
                r.destination, r.nexthop
            )));
        }
    }
    Ok(())
}

fn overlaps(a: IpNet, b: IpNet) -> bool {
    a.contains(&b.addr()) || b.contains(&a.addr())
}

fn is_router_device(d: &Option<Id>) -> bool {
    d.as_ref().is_some_and(|d| d.kind == kind::ROUTER)
}

fn prefix_len(net: IpNet) -> u8 {
    match net {
        IpNet::V4(n) => n.prefix_len(),
        IpNet::V6(n) => n.prefix_len(),
    }
}

/// A project resource may only reference resources in its own project or
/// cloud-wide (project `None`) ones.
fn check_same_project(mine: &Meta, theirs: &Meta, what: &str) -> ApiResult<()> {
    match (&mine.project, &theirs.project) {
        (_, None) => Ok(()),
        (Some(a), Some(b)) if a == b => Ok(()),
        _ => Err(ApiError::forbidden(format!(
            "{what} {} belongs to another project",
            theirs.id
        ))),
    }
}
