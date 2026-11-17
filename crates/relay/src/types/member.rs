// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A member: one backend address:port in a pool, and what the last probe
//! said about it.

use super::balancer::default_true;
use super::kinds;
use pistack_types::{Id, Kind, Meta, Phase, Resource};
use serde::{Deserialize, Serialize};
use std::net::IpAddr;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MemberSpec {
    pub pool: Id,
    pub address: IpAddr,
    pub port: u16,
    #[serde(default = "default_weight")]
    pub weight: u16,
    /// Serve existing connections but take no new ones.
    #[serde(default)]
    pub backup: bool,
    #[serde(default = "default_true")]
    pub admin_up: bool,
}

fn default_weight() -> u16 {
    1
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatingStatus {
    Online,
    Offline,
    /// Administratively down or being removed; finishing in-flight work.
    Draining,
    /// The pool has no health monitor; status is unknown and traffic flows.
    #[default]
    NoMonitor,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct MemberStatus {
    #[serde(default)]
    pub phase: Phase,
    #[serde(default)]
    pub operating_status: OperatingStatus,
    /// Consecutive probe results in the current direction, for rise/fall.
    #[serde(default)]
    pub streak: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_check: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}

resource!(
    /// One backend address:port in a pool.
    Member,
    MemberSpec,
    MemberStatus,
    kinds::MEMBER
);

impl Member {
    pub fn addr(&self) -> std::net::SocketAddr {
        std::net::SocketAddr::new(self.spec.address, self.spec.port)
    }
}
