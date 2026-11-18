// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Condition;
use serde::{Deserialize, Serialize};

use super::rest::Rest;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub(crate) struct MachineStatusView {
    #[serde(default)]
    pub(crate) addresses: Vec<String>,
    #[serde(default)]
    pub(crate) conditions: Vec<Condition>,
    #[serde(flatten)]
    pub(crate) rest: Rest,
}
