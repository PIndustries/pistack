// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::spec::probe_spec::ProbeSpec;
use serde::{Deserialize, Serialize};

/// Probes for one container, persisted by a runtime so a restarted agent
/// can keep checking without the original sandbox document.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContainerProbes {
    pub container: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub liveness: Option<ProbeSpec>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub readiness: Option<ProbeSpec>,
}
