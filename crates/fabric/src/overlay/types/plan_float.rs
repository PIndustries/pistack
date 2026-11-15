// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// A floating address to host on the network that lists it.
#[derive(Clone, Debug)]
pub struct PlanFloat {
    pub id: String,
    pub floating: String,
    pub fixed: String,
}
