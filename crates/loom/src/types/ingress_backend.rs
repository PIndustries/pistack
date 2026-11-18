// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::int_or_name::IntOrName;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct IngressBackend {
    /// Service name or id.
    pub service: String,
    /// Service port number or name.
    pub port: IntOrName,
}
