// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Whether a member is taking traffic right now.

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq, Hash, Default)]
#[serde(rename_all = "snake_case")]
pub enum OperatingStatus {
    Online,
    Offline,
    /// Administratively down or being removed; finishing in-flight work.
    Draining,
    /// The pool has no health monitor; status is unknown and traffic flows.
    #[default]
    NoMonitor,
}
