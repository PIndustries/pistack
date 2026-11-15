// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::dnat_rule::DnatRule;
use crate::overlay::types::overlay_plan::OverlayPlan;
use crate::overlay::types::sg_rule::SgRule;

#[test]
pub(in crate::overlay) fn plan_json_round_trips() {
    let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    leg.external_ip = Some("10.66.66.9".into());
    leg.external_dev = "wg-fleet".into();
    leg.metadata = true;
    leg.dnat = vec![DnatRule {
        id: "fip1".into(),
        floating: "10.66.66.80".into(),
        fixed: "10.90.0.5".into(),
    }];
    let mut guest = port(
        "port-01a50318-a507-74fd-a86e-5acbda974701",
        "52:54:00:00:00:22",
        "10.0.0.2",
        "10.90.0.22",
    );
    guest.ingress = vec![SgRule {
        proto: "tcp".into(),
        port_min: 22,
        port_max: 22,
        cidr: "10.66.66.0/24".into(),
    }];
    guest.egress = vec![SgRule {
        proto: "any".into(),
        port_min: 0,
        port_max: 0,
        cidr: "0.0.0.0/0".into(),
    }];
    let nets = [network(NET_A, 9, vec![leg], vec![guest])];
    let plan = plan(&host("10.0.0.1"), &nets, &["10.0.0.2".into()]).unwrap();
    let json = serde_json::to_string(&plan).unwrap();
    let back: OverlayPlan = serde_json::from_str(&json).unwrap();
    assert_eq!(plan, back);
    assert_eq!(back.version, 1);
    assert_eq!(back.networks[0].ports[0].ingress[0].proto, "tcp");
    assert!(back.networks[0].routers[0].snat);
    assert!(back.networks[0].provider.is_none());
    assert!(back.networks[0].floating.is_empty());
    assert!(back.networks[0].routers[0].interfaces[0].dhcp.is_empty());
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    let net = value["networks"][0].as_object_mut().unwrap();
    net.remove("provider");
    net.remove("floating");
    net["routers"][0]["interfaces"][0]
        .as_object_mut()
        .unwrap()
        .remove("dhcp");
    let old: OverlayPlan = serde_json::from_value(value).unwrap();
    assert!(old.networks[0].provider.is_none());
    assert!(old.networks[0].floating.is_empty());
    assert!(old.networks[0].routers[0].interfaces[0].dhcp.is_empty());
}
