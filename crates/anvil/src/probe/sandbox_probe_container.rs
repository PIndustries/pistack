// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::spec::probe_spec::ProbeSpec;
use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct SandboxProbeContainer {
    #[serde(default)]
    pub(crate) name: String,
    #[serde(default)]
    pub(crate) liveness_probe: Option<ProbeSpec>,
    #[serde(default)]
    pub(crate) readiness_probe: Option<ProbeSpec>,
}
