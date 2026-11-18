// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use ipnet::IpNet;
use pistack_types::Id;
use serde::{Deserialize, Serialize};

/// Who the rule matches on the far side. Exactly one of these.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum Remote {
    /// Any address of the rule's ethertype.
    #[default]
    Any,
    Cidr(IpNet),
    /// Members of another (or the same) security group.
    Group(Id),
}
