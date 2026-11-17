// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! A member create under `/pools/{id}/members`: the pool comes from the
//! path, so the body carries no name.

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct MemberBody {
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
