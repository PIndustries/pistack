// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Id;
use serde::{Deserialize, Serialize};

/// A machine that was handed back to the scheduler.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Evacuated {
    pub id: Id,
    pub name: String,
    /// The image it starts from again. Its root disk stayed on the host.
    pub rebuilt_from: String,
    /// Shared volumes that are plugged back in once it runs.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub volumes: Vec<String>,
}
