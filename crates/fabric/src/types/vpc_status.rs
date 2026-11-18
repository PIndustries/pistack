// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Id, Phase};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct VpcStatus {
    #[serde(default)]
    pub phase: Phase,
    /// The router this VPC owns. Made with the VPC, deleted with it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub router: Option<Id>,
    #[serde(default)]
    pub subnets: u32,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub message: String,
    #[serde(default)]
    pub observed_revision: u64,
}
