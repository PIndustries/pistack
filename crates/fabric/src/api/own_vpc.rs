// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use pistack_types::Id;

use crate::types::Network;

/// The vpc a network names in its spec.
pub(crate) fn own_vpc(c: &Client, n: &Network) -> Option<Id> {
    let _ = c;
    n.spec.vpc.clone()
}
