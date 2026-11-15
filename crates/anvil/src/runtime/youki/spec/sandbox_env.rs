// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SandboxEnv {
    #[serde(default)]
    pub(in crate::runtime::youki) name: String,
    #[serde(default)]
    pub(in crate::runtime::youki) value: String,
    #[serde(default)]
    pub(in crate::runtime::youki) value_from: Option<serde_json::Value>,
}
