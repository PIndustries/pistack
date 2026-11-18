// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::Deserialize;
use std::net::IpAddr;

#[derive(Deserialize)]
pub(crate) struct FloatingIpSpecView {
    #[serde(default)]
    pub(crate) address: Option<IpAddr>,
    #[serde(default)]
    pub(crate) port: Option<Id>,
}
