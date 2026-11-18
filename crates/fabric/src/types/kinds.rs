// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::kind;

/// Every kind fabric owns, for the reconciler's event filter.
pub const KINDS: &[&str] = &[
    kind::VPC,
    kind::NETWORK,
    kind::SUBNET,
    kind::PORT,
    kind::ROUTER,
    kind::SECGROUP,
    kind::FLOATING_IP,
];
