// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet::PodNetwork;
use crate::runtime::youki::network::nic_addresses;
use crate::runtime::youki::network::ObservedNet;
use crate::runtime::MachineRequest;

pub(in crate::runtime::youki) fn observe(req: &MachineRequest, net: &PodNetwork) -> ObservedNet {
    match net {
        PodNetwork::Linked(link) => ObservedNet {
            addresses: nic_addresses(req),
            message: String::new(),
            netns: link.netns.clone(),
            veths: link.veths.clone(),
            ovs: link.ovs.clone(),
        },
        PodNetwork::Refused(message) => ObservedNet {
            addresses: Vec::new(),
            message: message.clone(),
            netns: String::new(),
            veths: Vec::new(),
            ovs: Vec::new(),
        },
        PodNetwork::None => ObservedNet {
            addresses: nic_addresses(req),
            message: String::new(),
            netns: String::new(),
            veths: Vec::new(),
            ovs: Vec::new(),
        },
    }
}
