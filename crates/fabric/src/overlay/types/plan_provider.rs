// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Caller-supplied physnet for a provider network. Not a planned device.
#[derive(Clone, Debug)]
pub struct PlanProvider {
    pub physnet: String,
    pub vlan: Option<u16>,
}
