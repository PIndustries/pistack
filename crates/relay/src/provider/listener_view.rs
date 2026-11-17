// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A listener with everything a provider needs to program it.

use super::route_view::RouteView;
use crate::types::{Balancer, Listener, Member, Pool};

/// A listener with everything the provider needs to program it.
#[derive(Clone, Debug)]
pub struct ListenerView {
    pub balancer: Balancer,
    pub listener: Listener,
    /// Default pool and its members, when the listener has one. Members
    /// that are administratively down or `Offline` are already filtered
    /// out; what remains is what should take traffic.
    pub pool: Option<(Pool, Vec<Member>)>,
    /// Set for [`crate::types::ListenerProtocol::Terminated`] when `keep`
    /// already has that listener's certificate. Absent means TLS stays
    /// unavailable. The proxy does not generate one.
    pub tls: Option<crate::tls::TlsMaterial>,
    /// The listener's routes in spec order, each with its pool's active
    /// members filtered the same way as `pool`.
    pub routes: Vec<RouteView>,
}
