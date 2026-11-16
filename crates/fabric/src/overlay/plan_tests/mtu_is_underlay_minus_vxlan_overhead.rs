// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::types::plan_error::PlanError;

#[test]
pub(in crate::overlay) fn mtu_is_underlay_minus_vxlan_overhead() {
    let mut n = network(NET_A, 1, vec![], vec![]);
    let mut h = host("10.0.0.1");
    h.underlay_mtu = 0;
    assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1450);
    h.underlay_mtu = 1500;
    assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1450);
    h.underlay_mtu = 9000;
    assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1500);
    n.mtu = 1400;
    h.underlay_mtu = 1500;
    assert_eq!(plan(&h, &[n.clone()], &[]).unwrap().networks[0].mtu, 1400);
    n.mtu = 0;
    assert!(matches!(plan(&h, &[n], &[]).unwrap_err(), PlanError::Mtu));
}
