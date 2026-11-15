// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde::Serialize;

/// A floating address hosted in its own namespace on this segment.
///
/// `netns` is `nf` plus 12 hex digits of the floating id. `veth_bridge` is
/// `e` plus those digits, and `veth_ns` is `f` plus those digits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct FloatingLocal {
    pub id: String,
    pub floating: String,
    pub fixed: String,
    pub netns: String,
    pub veth_bridge: String,
    pub veth_ns: String,
}
