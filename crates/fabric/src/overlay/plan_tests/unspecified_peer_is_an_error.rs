// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::types::plan_error::PlanError;

#[test]
pub(in crate::overlay) fn unspecified_peer_is_an_error() {
    let nets = [network(NET_A, 1, vec![], vec![])];
    let err = plan(&host("10.0.0.1"), &nets, &["0.0.0.0".into()]).unwrap_err();
    assert!(matches!(err, PlanError::Ip(ref msg) if msg.contains("0.0.0.0")));
    for bad in [
        "255.255.255.255",
        "224.0.0.1",
        "239.255.255.255",
        "::1",
        "not-an-ip",
    ] {
        let nets = [network(NET_A, 1, vec![], vec![])];
        assert!(
            plan(&host("10.0.0.1"), &nets, &[bad.into()]).is_err(),
            "{bad}"
        );
    }
}
