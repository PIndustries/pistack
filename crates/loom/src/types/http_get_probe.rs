// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::default_probe_path::default_probe_path;
use super::probe_scheme::ProbeScheme;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpGetProbe {
    #[serde(default = "default_probe_path")]
    pub path: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<ProbeScheme>,
}
