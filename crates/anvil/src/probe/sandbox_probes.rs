// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::sandbox_probe_container::SandboxProbeContainer;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SandboxProbes {
    #[serde(default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) containers: Vec<SandboxProbeContainer>,
}
