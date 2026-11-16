// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::dnat_rule::DnatRule;
use crate::overlay::types::plan_error::PlanError;

#[test]
pub(in crate::overlay) fn ipv6_external_snat_is_a_private_ula() {
    let mut leg = router(RTR_A, "fd00:0090::0001", "fd00:0090::/64");
    leg.prefix = 64;
    leg.external_ip = Some("fd00:0066::0009".into());
    leg.external_dev = "external".into();
    leg.dnat = vec![DnatRule {
        id: "fip6".into(),
        floating: "fd00:0066::000a".into(),
        fixed: "fd00:0090::0010".into(),
    }];
    let nets = [network(NET_A, 11, vec![leg], vec![])];
    let got = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let ns = &got.networks[0].routers[0];
    assert!(ns.snat);
    assert_eq!(ns.external_ip.as_deref(), Some("fd00:66::9"));
    assert_eq!(ns.interfaces[0].cidr, "fd00:90::/64");
    assert_eq!(ns.interfaces[0].gateway, "fd00:90::1");
    assert_eq!(ns.dnat[0].floating, "fd00:66::a");
    assert_eq!(ns.dnat[0].fixed, "fd00:90::10");
    assert_eq!(got.host, "10.0.0.1");

    let mut ula = router(RTR_A, "fc00::1", "fc00::/64");
    ula.prefix = 64;
    ula.external_ip = Some("fc00:66::1".into());
    let nets = [network(NET_A, 12, vec![ula], vec![])];
    let got = plan(&host("10.1.0.1"), &nets, &[]).unwrap();
    assert!(got.networks[0].routers[0].snat);
    assert_eq!(
        got.networks[0].routers[0].external_ip.as_deref(),
        Some("fc00:66::1")
    );

    for bad in ["2001:db8::9", "fe80::1", "::1", "2001:db8:1::1"] {
        let mut leg = router(RTR_A, "fd00:90::1", "fd00:90::/64");
        leg.prefix = 64;
        leg.external_ip = Some(bad.into());
        let nets = [network(NET_A, 13, vec![leg], vec![])];
        let err = plan(&host("10.0.0.1"), &nets, &[]).unwrap_err();
        assert!(
            matches!(err, PlanError::Ip(ref msg) if msg.contains("not a private ipv6")),
            "{bad}: {err}"
        );
    }

    // An IPv6 address is not an underlay, public or unique-local.
    let nets = [network(NET_A, 1, vec![], vec![])];
    assert!(plan(&host("2001:db8::1"), &nets, &[]).is_err());
    let nets = [network(NET_A, 1, vec![], vec![])];
    assert!(plan(&host("fd00::1"), &nets, &[]).is_err());
    let nets = [network(NET_A, 1, vec![], vec![])];
    assert!(plan(&host("10.0.0.1"), &nets, &["2001:db8::2".into()]).is_err());

    let mut mixed = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    mixed.external_ip = Some("fd00:66::9".into());
    let nets = [network(NET_A, 14, vec![mixed], vec![])];
    let err = plan(&host("10.0.0.1"), &nets, &[]).unwrap_err();
    assert!(matches!(err, PlanError::Invalid(ref msg) if msg.contains("snat family")));
}
