// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::svc;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;
use crate::overlay::types::service_rule::ServiceRule;

#[test]
pub(in crate::overlay) fn service_rules_are_checked() {
    let bad = [
        svc("", "10.96.0.10", 80, &[]),
        svc("svc-a", "fd00::10", 80, &[]),
        svc("svc-a", "10.96.0.10", 0, &[]),
        svc("svc-a", "10.96.0.10", 80, &[("10.90.0.5", 0)]),
        ServiceRule {
            protocol: "sctp".into(),
            ..svc("svc-a", "10.96.0.10", 80, &[])
        },
    ];
    for rule in bad {
        let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
        leg.services = vec![rule.clone()];
        let nets = [network(NET_A, 3, vec![leg], vec![])];
        assert!(plan(&host("10.0.0.1"), &nets, &[]).is_err(), "{rule:?}");
    }
    let mut leg = router(RTR_A, "10.90.0.1", "10.90.0.0/24");
    leg.services = vec![
        svc("svc-a", "10.96.0.10", 80, &[]),
        svc("svc-b", "10.96.0.10", 80, &[]),
    ];
    let nets = [network(NET_A, 3, vec![leg], vec![])];
    assert!(plan(&host("10.0.0.1"), &nets, &[]).is_err());
}
