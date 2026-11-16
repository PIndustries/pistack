// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;

/// A machine the probe loop can reach right now.
#[derive(Clone, Debug)]
pub struct ProbeTarget {
    pub id: Id,
    pub power_on: bool,
    pub address: Option<String>,
}
