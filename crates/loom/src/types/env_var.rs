// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::{Deserialize, Serialize};

// ---- kinds and well-known keys ------------------------------------------------

use super::env_var_source::EnvVarSource;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EnvVar {
    pub name: String,
    #[serde(default)]
    pub value: String,
    /// Pull the value from a ConfigMap or Secret key (`{config_map|secret, name, key}`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value_from: Option<EnvVarSource>,
}
