// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::service_rule::ServiceRule;

pub(in crate::overlay) fn same_service(a: &ServiceRule, b: &ServiceRule) -> bool {
    a.address == b.address && a.protocol == b.protocol && a.port == b.port
}
