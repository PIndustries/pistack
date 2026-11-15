// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_host::PlanHost;
use crate::overlay::types::plan_network::PlanNetwork;
use crate::overlay::types::plan_port::PlanPort;
use crate::overlay::types::plan_router::PlanRouter;
use crate::overlay::types::service_backend::ServiceBackend;
use crate::overlay::types::service_rule::ServiceRule;

pub(crate) const NET_A: &str = "net-01a10318-a507-74fd-a86e-5acbda974722";
pub(crate) const NET_B: &str = "net-01a20318-b607-74fd-a86e-5acbda974733";
pub(crate) const RTR_A: &str = "rtr-01a30318-c507-74fd-a86e-5acbda974744";
pub(crate) const RTR_B: &str = "rtr-01a40318-d607-74fd-a86e-5acbda974755";

pub(crate) fn host(ip: &str) -> PlanHost {
    PlanHost {
        underlay: ip.into(),
        underlay_mtu: 0,
    }
}

pub(crate) fn network(
    id: &str,
    vni: u32,
    routers: Vec<PlanRouter>,
    ports: Vec<PlanPort>,
) -> PlanNetwork {
    PlanNetwork {
        id: id.into(),
        vni,
        mtu: 1500,
        ports,
        routers,
        provider: None,
        floating: vec![],
    }
}

pub(crate) fn router(id: &str, gateway: &str, cidr: &str) -> PlanRouter {
    PlanRouter {
        id: id.into(),
        gateway: gateway.into(),
        prefix: 24,
        cidr: cidr.into(),
        external_ip: None,
        external_dev: String::new(),
        dnat: Vec::new(),
        metadata: false,
        dhcp: vec![],
        services: vec![],
    }
}

pub(crate) fn svc(id: &str, address: &str, port: u16, backends: &[(&str, u16)]) -> ServiceRule {
    ServiceRule {
        id: id.into(),
        address: address.into(),
        protocol: "tcp".into(),
        port,
        backends: backends
            .iter()
            .map(|(a, p)| ServiceBackend {
                address: (*a).into(),
                port: *p,
            })
            .collect(),
    }
}

pub(crate) fn port(id: &str, mac: &str, underlay: &str, address: &str) -> PlanPort {
    PlanPort {
        id: id.into(),
        mac: mac.into(),
        address: address.into(),
        underlay: underlay.into(),
        ingress: Vec::new(),
        egress: Vec::new(),
        extra: Vec::new(),
    }
}
