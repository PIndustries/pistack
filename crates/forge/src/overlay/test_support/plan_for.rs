// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::overlay::OverlayPlan;
use serde_json::Value;

pub(in crate::overlay) fn plan_for(plans: &[(String, Value)], node: &str) -> OverlayPlan {
    let (_, value) = plans.iter().find(|(n, _)| n == node).unwrap();
    serde_json::from_value(value.clone()).unwrap()
}
