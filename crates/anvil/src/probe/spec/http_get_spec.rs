// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::default_path::default_path;
use crate::probe::spec::probe_scheme::ProbeScheme;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct HttpGetSpec {
    #[serde(default = "default_path")]
    pub path: String,
    pub port: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scheme: Option<ProbeScheme>,
}
