// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! What a provider reports once a balancer exists.

use std::net::IpAddr;

/// What the provider reports once a balancer exists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Realized {
    pub vip: IpAddr,
    /// Provider-side handle, stored in `status.provider_ref`.
    pub provider_ref: String,
}
