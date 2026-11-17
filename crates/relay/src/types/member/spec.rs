// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A member's request: the backend address:port and how much traffic it
//! should take.

use crate::types::default_true::default_true;
use crate::types::default_weight::default_weight;
use pistack_types::Id;
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
