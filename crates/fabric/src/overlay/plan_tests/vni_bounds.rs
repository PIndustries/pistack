// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::plan::plan;
use crate::overlay::tests::test_support::host;
use crate::overlay::tests::test_support::network;
use crate::overlay::tests::test_support::NET_A;
use crate::overlay::types::plan_error::PlanError;

#[test]
pub(in crate::overlay) fn vni_bounds() {
    let zero = [network(NET_A, 0, vec![], vec![])];
    assert!(matches!(
        plan(&host("10.0.0.1"), &zero, &[]).unwrap_err(),
        PlanError::Vni(0)
    ));
    let big = [network(NET_A, 16_777_216, vec![], vec![])];
    assert!(matches!(
        plan(&host("10.0.0.1"), &big, &[]).unwrap_err(),
        PlanError::Vni(16_777_216)
    ));
    let max = [network(NET_A, 16_777_215, vec![], vec![])];
    assert_eq!(
        plan(&host("10.0.0.1"), &max, &[]).unwrap().networks[0].vni,
        16_777_215
    );
}
