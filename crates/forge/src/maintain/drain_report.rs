// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::Host;
use serde::{Deserialize, Serialize};

use super::left::Left;
use super::moving::Moving;

/// What `POST /hosts/{id}/drain` answers.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct DrainReport {
    pub host: Host,
    /// Live migrations this call started, and ones still running from an
    /// earlier call. A machine leaves this list when its new host reports it.
    pub migrating: Vec<Moving>,
    /// Machines still on the host that this call did not move.
    pub not_moved: Vec<Left>,
}
