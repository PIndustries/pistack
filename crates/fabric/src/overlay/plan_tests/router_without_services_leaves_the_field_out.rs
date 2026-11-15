// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::router;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::tests::test_support::RTR_A;

#[test]
pub(in crate::overlay) fn router_without_services_leaves_the_field_out() {
    let nets = [network(
        NET_A,
        3,
        vec![router(RTR_A, "10.90.0.1", "10.90.0.0/24")],
        vec![],
    )];
    let plan = plan(&host("10.0.0.1"), &nets, &[]).unwrap();
    let json = serde_json::to_value(&plan.networks[0].routers[0]).unwrap();
    assert!(json.get("services").is_none());
}
