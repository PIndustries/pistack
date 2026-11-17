// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The `Balancer` resource: its spec and status bound to the `lb` kind.

mod provider;
mod spec;
mod status;
use pistack_types::{kind, Kind, Meta, Resource};
use serde::{Deserialize, Serialize};

pub use provider::Provider;
pub use spec::BalancerSpec;
pub use status::BalancerStatus;

resource!(
    /// A load balancer: one VIP, many listeners.
    Balancer,
    BalancerSpec,
    BalancerStatus,
    kind::BALANCER
);
