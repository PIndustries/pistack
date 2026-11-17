// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! The shapes a caller sends. `Create` and `Update` wrap any spec; a
//! member body carries no name because the pool and address name it.

use pistack_types::Id;
use serde::Deserialize;
use std::collections::BTreeMap;

#[derive(Deserialize)]
pub(super) struct Create<S> {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub labels: BTreeMap<String, String>,
    #[serde(default)]
    pub annotations: BTreeMap<String, String>,
    /// Admins may create in any project (or none); others inherit scope.
    #[serde(default)]
    pub project: Option<Id>,
    pub spec: S,
}

#[derive(Deserialize)]
pub(super) struct Update<S> {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub labels: Option<BTreeMap<String, String>>,
    #[serde(default)]
    pub annotations: Option<BTreeMap<String, String>>,
    pub spec: S,
}

/// Member create under `/pools/{id}/members`: the pool comes from the path.
#[derive(Deserialize)]
pub(super) struct MemberBody {
    pub address: std::net::IpAddr,
    pub port: u16,
    #[serde(default = "one")]
    pub weight: u16,
    #[serde(default)]
    pub backup: bool,
    #[serde(default = "yes")]
    pub admin_up: bool,
}

fn one() -> u16 {
    1
}
fn yes() -> bool {
    true
}

/// The query string every handler reads: who to act as, and what to
/// restrict the list to.
#[derive(Deserialize, Default)]
pub(super) struct ScopeQuery {
    /// Admin-only: act in this project instead of the token's.
    #[serde(default)]
    pub project: Option<Id>,
    /// Admin-only: list every project.
    #[serde(default)]
    pub all: Option<bool>,
    /// Listeners and pools: restrict to one balancer (name or id).
    #[serde(default)]
    pub balancer: Option<String>,
    /// `/members`: restrict to one pool (name or id).
    #[serde(default)]
    pub pool: Option<String>,
}
