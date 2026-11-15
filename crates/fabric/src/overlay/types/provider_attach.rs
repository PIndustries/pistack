// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde::Serialize;

/// How a provider segment attaches to a host physnet.
///
/// `link` is `pv` plus 12 hex digits of the network id.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProviderAttach {
    pub physnet: String,
    pub vlan: Option<u16>,
    pub link: String,
}
