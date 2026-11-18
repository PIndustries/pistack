// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

/// How the evacuation proved the host is off, so its machines can be
/// started elsewhere without leaving a second copy running.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Proof {
    /// The server cut the host's power through its BMC and read the off
    /// state back.
    PowerOff,
    /// An operator said so (`confirm_dead`), for a host with no power
    /// control the server can drive.
    Operator,
}

impl Proof {
    pub fn as_str(self) -> &'static str {
        match self {
            Proof::PowerOff => "power-off",
            Proof::Operator => "operator",
        }
    }

    /// The proof in the shape an audit `action` takes: lowercase, words
    /// joined with underscores.
    pub(crate) fn audit_tag(self) -> &'static str {
        match self {
            Proof::PowerOff => "power_off",
            Proof::Operator => "operator",
        }
    }
}
