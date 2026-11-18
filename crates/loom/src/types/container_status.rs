// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::container_state::ContainerState;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ContainerStatus {
    pub name: String,
    pub image: String,
    pub ready: bool,
    #[serde(default)]
    pub restart_count: u32,
    #[serde(default)]
    pub state: ContainerState,
}
