// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The pool: its members' protocol, the algorithm that spreads new
//! connections, and how long a client sticks to one member.

use super::balancer::default_true;
use super::health_monitor::HealthMonitor;
use super::kinds;
use pistack_types::{Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum PoolProtocol {
    #[default]
    Tcp,
    Udp,
    Http,
    Https,
}

/// How a pool spreads new connections. Both data planes run these through
/// [`crate::picker`], so a pool balances the same over TCP and over HTTP.
/// A value outside the three below is refused when the pool is read.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum Algorithm {
    /// Each member is visited a number of times in proportion to its
    /// weight. The default.
    #[default]
    #[serde(deserialize_with = "crate::picker::wire::deserialize")]
    RoundRobin,
    /// The healthy member with the fewest open connections. A tie goes to
    /// the heavier member, then to the earlier one.
    #[serde(deserialize_with = "crate::picker::wire::deserialize")]
    LeastConn,
    /// One client address stays on one healthy member while the member set
    /// is unchanged.
    #[serde(deserialize_with = "crate::picker::wire::deserialize")]
    SourceIp,
}

impl<'de> Deserialize<'de> for Algorithm {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        crate::picker::wire::deserialize(deserializer)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SessionPersistence {
    SourceIp,
    HttpCookie,
    AppCookie { cookie: String },
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct PoolSpec {
    pub balancer: Id,
    #[serde(default)]
    pub protocol: PoolProtocol,
    #[serde(default)]
    pub algorithm: Algorithm,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_persistence: Option<SessionPersistence>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub health_monitor: Option<HealthMonitor>,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct PoolStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider_ref: Option<String>,
    /// Members `Online` at the last check.
    #[serde(default)]
    pub online: u32,
    #[serde(default)]
    pub members: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// A set of members with a balancing algorithm and a health monitor.
    Pool,
    PoolSpec,
    PoolStatus,
    kinds::POOL
);
