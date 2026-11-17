// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{kind, Id};

/// Rank for deleting managed orphans. Lower goes first so dependents leave
/// before the rows they hold (record sets and listeners, then pods and
/// machines, then routers, subnets, and networks).
pub(crate) fn delete_rank(id: &str) -> u8 {
    if Id::parse(kind::RECORD, id).is_some() {
        0
    } else if Id::parse(relay::kinds::LISTENER, id).is_some() {
        1
    } else if Id::parse(kind::POD, id).is_some() {
        2
    } else if Id::parse(kind::MACHINE, id).is_some() {
        3
    } else if Id::parse(kind::ROUTER, id).is_some() {
        4
    } else if Id::parse(kind::SUBNET, id).is_some() {
        5
    } else if Id::parse(kind::NETWORK, id).is_some() {
        6
    } else {
        7
    }
}
