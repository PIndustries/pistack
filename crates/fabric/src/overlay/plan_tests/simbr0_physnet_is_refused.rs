// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::types::plan_provider::PlanProvider;

#[test]
pub(in crate::overlay) fn simbr0_physnet_is_refused() {
    let mut provider = network(NET_A, 0, vec![], vec![]);
    provider.provider = Some(PlanProvider {
        physnet: "simbr0".into(),
        vlan: None,
    });
    assert!(plan(&host("10.0.0.1"), &[provider], &[]).is_err());
    let mut empty = network(NET_A, 0, vec![], vec![]);
    empty.provider = Some(PlanProvider {
        physnet: String::new(),
        vlan: None,
    });
    assert!(plan(&host("10.0.0.1"), &[empty], &[]).is_err());
}
