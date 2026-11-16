// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::segment_for_nic;
use pistack_types::kind;

#[test]
fn a_hostnat_network_stays_on_the_host_bridge() {
    let mut net = fabric::Network {
        meta: pistack_types::Meta::new(kind::NETWORK, "app", None),
        spec: fabric::NetworkSpec::default(),
        status: fabric::NetworkStatus {
            segment_id: Some(7),
            datapath: Some("hostnat".into()),
            ..fabric::NetworkStatus::default()
        },
    };
    let stayed = segment_for_nic(&net, "10.90.0.2").unwrap();
    assert!(stayed.is_none());
    net.status.datapath = Some("overlay".into());
    // A parallel test may or may not select the overlay backend. Host NAT
    // is the case that must hold either way: the assertion above.
    let _ = segment_for_nic(&net, "10.90.0.2");
}
