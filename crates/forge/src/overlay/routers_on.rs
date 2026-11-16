// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{
    dhcp_leases, external_for, follows_port, service_rules, Guest, ServiceRecord,
};
use fabric::overlay::{DnatRule, PlanRouter};
use fabric::{FloatingIp, Network, Port, Router, Subnet};
use pistack_types::{ApiError, ApiResult, Id};

#[allow(clippy::too_many_arguments)]
pub(in crate::overlay) fn routers_on(
    network: &Id,
    networks: &[Network],
    subnets: &[Subnet],
    routers: &[Router],
    ports: &[Port],
    fips: &[FloatingIp],
    services: &[ServiceRecord],
    bound: &[&Guest],
) -> ApiResult<Vec<PlanRouter>> {
    let on_net: Vec<&Subnet> = subnets
        .iter()
        .filter(|s| &s.spec.network == network)
        .collect();
    let mut out = Vec::new();
    for router in routers {
        let legs: Vec<&Subnet> = on_net
            .iter()
            .copied()
            .filter(|s| router.spec.interfaces.iter().any(|id| id == &s.meta.id))
            .collect();
        if legs.is_empty() {
            continue;
        }
        if legs.len() > 1 {
            return Err(ApiError::unprocessable(format!(
                "router {} has more than one subnet on {network}",
                router.meta.id
            )));
        }
        let subnet = legs[0];
        // IPv6 is a real leg. Only the shared host-NAT prefix is skipped,
        // and that network never reaches here.
        if subnet.spec.cidr.to_string() == "10.88.0.0/24" {
            continue;
        }
        let Some(gateway) = subnet.spec.gateway else {
            continue;
        };
        let external_ip = external_for(router, subnet, subnets);
        let dnat = if external_ip.as_ref().is_some_and(|ip| ip.contains(':')) {
            // IPv6 SNAT does not rewrite an IPv4 floating address.
            Vec::new()
        } else {
            fips.iter()
                .filter(|f| {
                    f.spec.port.as_ref().is_some_and(|p| {
                        bound.iter().any(|b| &b.port.meta.id == p)
                            && (f.status.router.as_ref() == Some(&router.meta.id)
                                || f.status.router.is_none())
                    })
                })
                .filter(|f| {
                    networks
                        .iter()
                        .find(|n| n.meta.id == f.spec.network)
                        .is_none_or(|n| !follows_port(n, subnets))
                })
                .filter_map(|f| {
                    let floating = f.spec.address.filter(|a| a.is_ipv4())?.to_string();
                    if external_ip.as_ref() == Some(&floating) {
                        return None;
                    }
                    let fixed = f
                        .status
                        .fixed_ip
                        .filter(|a| a.is_ipv4())
                        .map(|a| a.to_string())
                        .or_else(|| {
                            ports
                                .iter()
                                .find(|p| Some(&p.meta.id) == f.spec.port.as_ref())
                                .and_then(|p| {
                                    p.spec
                                        .fixed_ips
                                        .iter()
                                        .find(|ip| ip.is_ipv4())
                                        .map(|ip| ip.to_string())
                                })
                        })?;
                    Some(DnatRule {
                        id: f.meta.id.to_string(),
                        floating,
                        fixed,
                    })
                })
                .collect()
        };
        out.push(PlanRouter {
            id: router.meta.id.to_string(),
            gateway: gateway.to_string(),
            prefix: subnet.spec.cidr.prefix_len(),
            cidr: subnet.spec.cidr.to_string(),
            external_ip: external_ip.clone(),
            external_dev: if external_ip.is_some() {
                "external".into()
            } else {
                String::new()
            },
            dnat,
            metadata: true,
            dhcp: dhcp_leases(subnet, bound),
            services: service_rules(router, subnet, services),
        });
    }
    Ok(out)
}
