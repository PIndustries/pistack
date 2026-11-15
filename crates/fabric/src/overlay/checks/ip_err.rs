// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::overlay::types::plan_error::PlanError;

pub(in crate::overlay) fn ip_err(role: &str, raw: &str) -> PlanError {
    PlanError::Ip(format!("{role} {raw} is not a usable ipv4 address"))
}
