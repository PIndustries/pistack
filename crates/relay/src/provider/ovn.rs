// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! OVN L4 provider. Today a stub that returns `not_implemented`; this file
//! documents the northbound mapping it will write so the OVSDB client work
//! (shared with `fabric`) has a target.
//!
//! # Mapping to the OVN northbound schema
//!
//! One PiStack `Balancer` becomes one `Load_Balancer` row per listener
//! protocol, because `Load_Balancer.protocol` is a single value (`tcp`,
//! `udp` or `sctp`). The row's `vips` map is the whole listener/pool
//! picture for that protocol:
//!
//! ```text
//! Load_Balancer
//!   name        = "pistack-<balancer-id>-<proto>"
//!   protocol    = "tcp" | "udp"
//!   vips        = { "<vip>:<listener.port>" : "<m1.addr>:<m1.port>,<m2.addr>:<m2.port>" }
//!   external_ids = { "pistack.io/balancer": "<id>", "pistack.io/listener:<port>": "<listener-id>" }
//!   options     = { "reject": "true" }        # RST when the member list is empty
//!   selection_fields = ["ip_src", "ip_dst", "tp_src", "tp_dst"]   # default 5-tuple hash
//!                     ["ip_src"]                                    # Algorithm::SourceIp / SessionPersistence::SourceIp
//! ```
//!
//! * `Algorithm::RoundRobin` is what OVN's hashing gives; `LeastConn` is
//!   not expressible and is reported as `status.message` with RoundRobin
//!   applied. `SourceIp` maps to `selection_fields`.
//! * Members in `vips` are only those `Online` (or `NoMonitor`). relay's
//!   own `health.rs` drives this; OVN's `Load_Balancer_Health_Check` is an
//!   alternative when the chassis can reach members directly, and is left
//!   for later.
//! * `Member.weight` has no OVN equivalent; weights other than 1 are
//!   approximated by repeating the member (capped) and noted in status.
//!
//! The row is attached where traffic enters:
//!
//! ```text
//! Logical_Switch.load_balancer += <lb>      for the VIP network's switch
//! Logical_Router.load_balancer += <lb>      for every router with an interface on it
//! ```
//!
//! so east-west traffic to the VIP is balanced on the switch and traffic
//! from other subnets (and a floating IP) on the router.
//!
//! The VIP itself is a `Logical_Switch_Port` of type `""` with no chassis
//! binding, created through `fabric::Client::create_port` with the
//! balancer as `device`, so IPAM, security groups and the floating-IP
//! association (`spec.floating_ip` -> `fabric` FIP pointing at that port)
//! all come from fabric unchanged. `Realized.provider_ref` is the
//! `Load_Balancer._uuid` of the TCP row (or the UDP row if there is no TCP
//! listener).
//!
//! Parked: this stub serves no provider value, so a balancer can never be
//! routed here. L4 balancers go to [`super::ForwardProvider`] and HTTP
//! listeners to [`super::RelayL7Provider`].

use super::{LbProvider, ListenerView, Realized};
use crate::types::{Balancer, Provider};
use async_trait::async_trait;
use pistack_types::{ApiError, ApiResult, Id};

/// Placeholder until the OVSDB client lands.
#[derive(Default)]
pub struct OvnProvider {
    /// OVN northbound connection string, e.g. `tcp:10.0.0.1:6641`.
    pub nb_addr: String,
}

impl OvnProvider {
    pub fn new(nb_addr: impl Into<String>) -> Self {
        Self {
            nb_addr: nb_addr.into(),
        }
    }

    fn todo(&self, what: &str) -> ApiError {
        ApiError::not_implemented(format!(
            "ovn load balancer provider ({}): {what} not implemented yet",
            self.nb_addr
        ))
    }
}

#[async_trait]
impl LbProvider for OvnProvider {
    fn name(&self) -> &'static str {
        "ovn"
    }

    fn serves(&self, _provider: Provider) -> bool {
        false
    }

    async fn ensure_balancer(&self, _lb: &Balancer) -> ApiResult<Realized> {
        Err(self.todo("ensure_balancer"))
    }

    async fn delete_balancer(&self, _id: &Id) -> ApiResult<()> {
        Err(self.todo("delete_balancer"))
    }

    async fn ensure_listener(&self, view: &ListenerView) -> ApiResult<String> {
        if view.listener.spec.protocol.is_l7() {
            return Err(ApiError::bad_request(
                "ovn balances L4 only; use provider relay for http listeners",
            ));
        }
        Err(self.todo("ensure_listener"))
    }

    async fn delete_listener(&self, _balancer: &Id, _listener: &Id) -> ApiResult<()> {
        Err(self.todo("delete_listener"))
    }
}
