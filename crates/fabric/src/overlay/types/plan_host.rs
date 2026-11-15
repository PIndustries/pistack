// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// The host the plan is for.
#[derive(Clone, Debug)]
pub struct PlanHost {
    pub underlay: String,
    /// `0` means 1500.
    pub underlay_mtu: u16,
}
