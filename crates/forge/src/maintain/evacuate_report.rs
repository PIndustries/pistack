// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use serde::{Deserialize, Serialize};

use super::evacuated::Evacuated;
use super::left::Left;
use super::proof::Proof;

/// What `POST /hosts/{id}/evacuate` answers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct EvacuateReport {
    pub host: Host,
    /// How this call proved the host is off: a power-off the server
    /// confirmed, or the operator's word.
    pub proof: Proof,
    /// Set when a power-off was tried and did not happen, saying why. The
    /// proof is then `operator` and the machines moved on that.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub power_off_failed: Option<String>,
    pub evacuated: Vec<Evacuated>,
    /// Machines left on the host's record, and why.
    pub skipped: Vec<Left>,
}
