// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::pub_consts::GATEWAY_CHASSIS_ENV;

pub(in crate::backend::ovn) fn chassis_from_env() -> Vec<String> {
    std::env::var(GATEWAY_CHASSIS_ENV)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|c| !c.is_empty())
        .map(str::to_string)
        .collect()
}
