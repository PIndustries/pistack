// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Deserialize, Default)]
pub(crate) struct MachineStatusView {
    #[serde(default)]
    pub(crate) addresses: Vec<String>,
}
