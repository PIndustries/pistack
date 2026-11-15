// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::podnet;
use crate::runtime::podnet::PodNetwork;
use crate::runtime::youki::network::ip_privilege;
use crate::runtime::youki::privilege::RunMode;

pub(in crate::runtime::youki) async fn drop_link(mode: &RunMode, net: &PodNetwork) {
    if let PodNetwork::Linked(link) = net {
        if let Err(err) =
            podnet::release(&ip_privilege(mode), &link.netns, &link.veths, &link.ovs).await
        {
            tracing::warn!(
                netns = %link.netns,
                error = %err,
                "youki runtime: network teardown failed"
            );
        }
    }
}
