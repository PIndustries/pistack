// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::{real_vni, shared_nat_network, Realised};
use fabric::overlay::PlanProvider;
use fabric::{Network, NetworkKind, Subnet};

pub(in crate::overlay) fn classify(net: &Network, subnets: &[Subnet]) -> Option<Realised> {
    match &net.spec.kind {
        NetworkKind::Provider { physnet, vlan } => Some(Realised::Provider(PlanProvider {
            physnet: physnet.clone(),
            vlan: *vlan,
        })),
        NetworkKind::Overlay => {
            if shared_nat_network(net, subnets) {
                return None;
            }
            let vni = real_vni(net)?;
            if net.spec.external {
                Some(Realised::External(vni))
            } else {
                Some(Realised::Tenant(vni))
            }
        }
    }
}
