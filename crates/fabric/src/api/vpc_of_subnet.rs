// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::Id;

use crate::types::Subnet;

/// The vpc a subnet's network belongs to.
pub(crate) fn vpc_of_subnet(c: &Client, s: &Subnet) -> Option<Id> {
    c.networks().get(&s.spec.network).ok()?.spec.vpc
}
