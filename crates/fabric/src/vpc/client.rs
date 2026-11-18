// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::{Network, NetworkSpec, PortSpec, Router, RouterSpec, Subnet, Vpc, VpcSpec};
use crate::usage;
use pistack_types::{kind, ApiError, ApiResult, Id, Meta};

use super::inside::inside;
use super::joined::joined;
use super::overlaps::overlaps;
use super::router_address::router_address;
use super::same_project::same_project;
use super::store_err::store_err;
use super::validate_cidrs::validate_cidrs;

impl Client {
    // ---- the vpc -----------------------------------------------------------

    /// A VPC comes with the router it owns. That router is not counted
    /// against the project's `routers` limit.
    pub fn create_vpc(&self, mut meta: Meta, spec: VpcSpec) -> ApiResult<Vpc> {
        meta.id = Id::new(kind::VPC);
        validate_cidrs(&spec.cidrs)?;
        crate::client::validate_routes(&spec.routes)?;
        let project = meta.project.clone();
        let vpc = usage::admitted(self.store(), project.as_ref(), usage::VPCS, || {
            self.vpcs()
                .create(Vpc {
                    meta: meta.clone(),
                    spec: spec.clone(),
                    status: Default::default(),
                })
                .map_err(store_err)
        })?;
        let mut rmeta = Meta::new(
            kind::ROUTER,
            format!("{}-router", vpc.meta.name),
            vpc.meta.project.clone(),
        );
        rmeta.description = format!("router owned by vpc {}", vpc.meta.name);
        rmeta
            .annotations
            .insert("vpc".into(), vpc.meta.name.clone());
        let made = self.router_row(
            rmeta,
            RouterSpec {
                vpc: Some(vpc.meta.id.clone()),
                ..RouterSpec::default()
            },
        );
        let rtr = match made {
            Ok(r) => r,
            Err(e) => {
                let _ = self.vpcs().delete(&vpc.meta.id);
                usage::mirror(self.store(), project.as_ref(), usage::VPCS);
                return Err(e);
            }
        };
        let out = self
            .vpcs()
            .modify(&vpc.meta.id, |v| {
                v.status.router = Some(rtr.meta.id.clone());
                Ok(())
            })
            .map_err(store_err)?;
        usage::mirror(self.store(), project.as_ref(), usage::VPCS);
        Ok(out)
    }

    /// Blocks may be added; one comes out only when no subnet uses it. The
    /// external network may be set, changed or cleared.
    pub fn update_vpc(
        &self,
        id: &Id,
        expected: Option<u64>,
        f: impl FnOnce(&mut Vpc) -> ApiResult<()>,
    ) -> ApiResult<Vpc> {
        let cur = self.vpcs().get(id).map_err(store_err)?;
        let mut next = cur.clone();
        f(&mut next)?;
        next.meta.id = cur.meta.id.clone();
        next.meta.project = cur.meta.project.clone();
        next.status = cur.status.clone();
        validate_cidrs(&next.spec.cidrs)?;
        crate::client::validate_routes(&next.spec.routes)?;
        for gone in cur
            .spec
            .cidrs
            .iter()
            .filter(|c| !next.spec.cidrs.contains(c))
        {
            if let Some(s) = self
                .subnets_of_vpc(id)?
                .into_iter()
                .find(|s| inside(*gone, s.spec.cidr))
            {
                return Err(ApiError::conflict(format!(
                    "subnet {} holds {} inside {gone}",
                    s.meta.name, s.spec.cidr
                )));
            }
        }
        if next.spec.external_network.is_none() && cur.spec.external_network.is_some() {
            self.vpc_external_clearable(id)?;
        }
        let rtr = self.vpc_router(id)?;
        let wants_gw = next.spec.external_network != rtr.spec.external_network;
        let wants_routes = next.spec.routes != rtr.spec.routes;
        if wants_gw || wants_routes {
            let net = match &next.spec.external_network {
                Some(n) => Some(self.external_network(n)?),
                None => None,
            };
            let old = rtr.spec.external_network.clone();
            let want = next.spec.external_network.clone();
            let routes = next.spec.routes.clone();
            let rid = rtr.meta.id.clone();
            self.routers()
                .modify(&rid, |r| {
                    r.spec.external_network = want.clone();
                    r.spec.routes = routes.clone();
                    Ok(())
                })
                .map_err(store_err)?;
            if wants_gw {
                for p in self.ports_of_device(&rid)? {
                    if Some(&p.spec.network) == old.as_ref() {
                        self.delete_port_unchecked(&p)?;
                    }
                }
                if let Some(net) = net {
                    let r = self.routers().get(&rid).map_err(store_err)?;
                    self.create_gateway_port(&r, &net)?;
                }
            }
        }
        self.vpcs().update(next, expected).map_err(store_err)
    }

    /// Only while the VPC holds no subnet. Its router goes with it.
    pub fn delete_vpc(&self, id: &Id) -> ApiResult<Vpc> {
        let vpc = self.vpcs().get(id).map_err(store_err)?;
        let held = self.subnets_of_vpc(id)?;
        if !held.is_empty() {
            return Err(ApiError::conflict(format!(
                "vpc {} still has {} subnet(s), starting with '{}'",
                vpc.meta.name,
                held.len(),
                held[0].meta.name
            )));
        }
        if let Some(r) = &vpc.status.router {
            self.delete_owned_router(r)?;
        }
        let out = self.vpcs().delete(id).map_err(store_err)?;
        usage::mirror(self.store(), out.meta.project.as_ref(), usage::VPCS);
        Ok(out)
    }

    /// Take a router a VPC owns out of the world, ports and all, without
    /// touching the `routers` count.
    pub(crate) fn delete_owned_router(&self, id: &Id) -> ApiResult<Router> {
        let router = self.routers().get(id).map_err(store_err)?;
        if router.spec.vpc.is_none() {
            return Err(ApiError::bad_request(format!(
                "router {} belongs to no vpc",
                router.meta.id
            )));
        }
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
        self.routers().delete(id).map_err(store_err)
    }

    pub(crate) fn vpc_external_clearable(&self, id: &Id) -> ApiResult<()> {
        let ext = self.vpc_router(id)?.spec.external_network.clone();
        for f in self.floating_ips().list(None).map_err(store_err)? {
            let depends = f.spec.port.is_some()
                && (ext.as_ref() == Some(&f.spec.network)
                    || self
                        .networks()
                        .get(&f.spec.network)
                        .map_err(store_err)?
                        .spec
                        .vpc
                        .as_ref()
                        == Some(id));
            if depends {
                let addr = f
                    .spec
                    .address
                    .map(|a| a.to_string())
                    .unwrap_or_else(|| f.meta.id.to_string());
                return Err(ApiError::conflict(format!(
                    "floating ip {addr} still reaches out through vpc {id}"
                )));
            }
        }
        Ok(())
    }

    // ---- membership --------------------------------------------------------

    /// Rule 4 and rule 8: the vpc a network names exists and is in the same
    /// project; a network in a vpc is neither external nor a provider network.
    pub(crate) fn check_vpc_membership(
        &self,
        mine: &Option<Id>,
        spec: &NetworkSpec,
    ) -> ApiResult<()> {
        let Some(id) = &spec.vpc else {
            return Ok(());
        };
        let vpc = self
            .vpcs()
            .get(id)
            .map_err(|_| ApiError::not_found(format!("vpc {id} not found in this project")))?;
        same_project(mine, &vpc.meta.project, "vpc")?;
        if spec.external {
            return Err(ApiError::bad_request(
                "a network in a vpc is private: it cannot be external",
            ));
        }
        if matches!(spec.kind, crate::types::NetworkKind::Provider { .. }) {
            return Err(ApiError::bad_request(
                "a network in a vpc cannot be a provider network",
            ));
        }
        Ok(())
    }

    /// The VPC a network belongs to, when it belongs to one.
    pub(crate) fn vpc_of_network(&self, network: &Id) -> Option<Vpc> {
        let net = self.networks().get(network).ok()?;
        self.vpcs().get(net.spec.vpc.as_ref()?).ok()
    }

    /// The router a VPC owns, refusing a VPC that has somehow lost it.
    pub(crate) fn vpc_router(&self, vpc: &Id) -> ApiResult<Router> {
        self.routers()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .find(|r| r.spec.vpc.as_ref() == Some(vpc))
            .ok_or_else(|| ApiError::unavailable(format!("vpc {vpc} has no router")))
    }

    /// Every subnet of the VPC, oldest first.
    pub fn subnets_of_vpc(&self, vpc: &Id) -> ApiResult<Vec<Subnet>> {
        let nets: Vec<Id> = self
            .networks()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|n| n.spec.vpc.as_ref() == Some(vpc))
            .map(|n| n.meta.id)
            .collect();
        let mut out: Vec<Subnet> = self
            .subnets()
            .list(None)
            .map_err(store_err)?
            .into_iter()
            .filter(|s| nets.contains(&s.spec.network))
            .collect();
        out.sort_by(|a, b| a.meta.id.cmp(&b.meta.id));
        Ok(out)
    }

    /// A subnet on a VPC network sits inside one of the VPC's blocks and
    /// overlaps no other subnet of it. One project throughout.
    pub(crate) fn check_vpc_subnet(&self, subnet: &Subnet) -> ApiResult<()> {
        let Some(vpc) = self.vpc_of_network(&subnet.spec.network) else {
            return Ok(());
        };
        same_project(&subnet.meta.project, &vpc.meta.project, "vpc")?;
        if !vpc.spec.cidrs.iter().any(|b| inside(*b, subnet.spec.cidr)) {
            return Err(ApiError::bad_request(format!(
                "{} is outside the blocks of vpc {}: {}",
                subnet.spec.cidr,
                vpc.meta.id,
                joined(&vpc.spec.cidrs)
            )));
        }
        for other in self.subnets_of_vpc(&vpc.meta.id)? {
            if other.meta.id != subnet.meta.id && overlaps(subnet.spec.cidr, other.spec.cidr) {
                return Err(ApiError::conflict(format!(
                    "{} overlaps subnet {} ({}) in vpc {}",
                    subnet.spec.cidr, other.meta.name, other.spec.cidr, vpc.meta.id
                )));
            }
        }
        Ok(())
    }

    /// What a vpc subnet needs before its row can be written: a free block
    /// and a router that can still take it. Checked first, so the common
    /// refusals happen while nothing at all is stored.
    pub(crate) fn vpc_joinable(&self, network: &Network, subnet: &Subnet) -> ApiResult<()> {
        let Some(vpc) = self.vpc_of_network(&network.meta.id) else {
            return Ok(());
        };
        let rtr = self.vpc_router(&vpc.meta.id)?;
        if rtr.spec.interfaces.contains(&subnet.meta.id) {
            return Ok(());
        }
        if self.backend().name() == "hostnat" {
            let held = self.subnets_of_vpc(&vpc.meta.id)?;
            if !held.is_empty() {
                return Err(ApiError::conflict(format!(
                    "one host holds no routing between networks: vpc {} keeps to one subnet, \
                     and it already has '{}' on {}",
                    vpc.meta.id, held[0].meta.name, held[0].spec.cidr
                )));
            }
        }
        router_address(subnet.spec.cidr, subnet.spec.gateway)
            .map(|_| ())
            .ok_or_else(|| {
                ApiError::bad_request(format!(
                    "{} has no address a router could take",
                    subnet.spec.cidr
                ))
            })
    }

    /// The subnet's interface on the vpc's router. Its port is made first,
    /// holding the subnet's router address, then the subnet is named on the
    /// router. Returns once both hold; the caller takes the port back if the
    /// subnet itself cannot be stored.
    pub(crate) fn vpc_attach(&self, vpc: &Vpc, subnet: &Subnet) -> ApiResult<()> {
        let rtr = self.vpc_router(&vpc.meta.id)?;
        if rtr.spec.interfaces.contains(&subnet.meta.id) {
            return Ok(());
        }
        let address = router_address(subnet.spec.cidr, subnet.spec.gateway).ok_or_else(|| {
            ApiError::bad_request(format!(
                "{} has no address a router could take",
                subnet.spec.cidr
            ))
        })?;
        let mut meta = Meta::new(
            kind::PORT,
            format!("{}-{}", rtr.meta.id, subnet.meta.id),
            subnet.meta.project.clone(),
        );
        meta.description = format!(
            "router {} interface on subnet {}",
            rtr.meta.name, subnet.meta.name
        );
        self.port_row(
            meta,
            PortSpec {
                network: subnet.spec.network.clone(),
                subnet: Some(subnet.meta.id.clone()),
                fixed_ips: vec![address],
                mac: String::new(),
                device: Some(rtr.meta.id.clone()),
                security_groups: Vec::new(),
                port_security: false,
                allowed_address_pairs: Vec::new(),
            },
            Some(subnet),
        )?;
        let sid = subnet.meta.id.clone();
        self.routers()
            .modify(&rtr.meta.id, |r| {
                if !r.spec.interfaces.contains(&sid) {
                    r.spec.interfaces.push(sid.clone());
                }
                Ok(())
            })
            .map_err(store_err)?;
        Ok(())
    }

    /// The subnet's interface goes with the subnet: the port first, since it
    /// holds an address inside the subnet.
    pub(crate) fn vpc_detach(&self, vpc: &Vpc, subnet: &Id) -> ApiResult<()> {
        let Ok(rtr) = self.vpc_router(&vpc.meta.id) else {
            return Ok(());
        };
        for p in self
            .ports_of_device(&rtr.meta.id)?
            .into_iter()
            .filter(|p| p.spec.subnet.as_ref() == Some(subnet))
        {
            self.delete_port_unchecked(&p)?;
        }
        if rtr.spec.interfaces.contains(subnet) {
            let want = rtr
                .spec
                .interfaces
                .iter()
                .filter(|i| *i != subnet)
                .cloned()
                .collect::<Vec<Id>>();
            self.routers()
                .modify(&rtr.meta.id, |r| {
                    r.spec.interfaces = want.clone();
                    Ok(())
                })
                .map_err(store_err)?;
        }
        Ok(())
    }

    /// Keep `status.subnets` at the number of subnets the VPC holds.
    pub(crate) fn count_vpc_subnets(&self, vpc: &Id) {
        let col = self.vpcs();
        let Ok(v) = col.get(vpc) else { return };
        let Ok(held) = self.subnets_of_vpc(vpc) else {
            return;
        };
        let n = held.len() as u32;
        if n == v.status.subnets {
            return;
        }
        if let Err(e) = col.modify(vpc, |v| {
            v.status.subnets = n;
            Ok(())
        }) {
            tracing::debug!(%vpc, error = %e, "fabric: vpc subnet count not recorded");
        }
    }
}
