// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::port;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::types::plan_error::PlanError;

#[test]
pub(in crate::overlay) fn loopback_and_public_underlay_are_usable() {
    let nets = [network(NET_A, 1, vec![], vec![])];
    assert!(plan(&host("127.0.0.1"), &nets, &[]).is_ok());
    let nets = [network(NET_A, 1, vec![], vec![])];
    assert!(plan(&host("8.8.8.8"), &nets, &["100.64.0.1".into()]).is_ok());
    let nets = [network(NET_A, 1, vec![], vec![])];
    assert!(plan(&host("0.0.0.0"), &nets, &[]).is_err());
    let ports = vec![port(
        "port-01a50318-a507-74fd-a86e-5acbda974701",
        "52:54:00:00:00:11",
        "",
        "10.90.0.11",
    )];
    let nets = [network(NET_A, 1, vec![], ports)];
    let err = plan(&host("10.0.0.1"), &nets, &[]).unwrap_err();
    assert!(matches!(err, PlanError::Invalid(ref msg) if msg.contains("empty underlay")));
}
