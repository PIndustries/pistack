// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::spec::SandboxEnv;
use crate::runtime::youki::spec::SandboxMount;
use serde::Deserialize;

#[derive(Clone, Debug, Default, Deserialize)]
pub struct SandboxContainer {
    #[serde(default)]
    pub(in crate::runtime::youki) name: String,
    #[serde(default)]
    pub(in crate::runtime::youki) image: String,
    #[serde(default)]
    pub(in crate::runtime::youki) command: Vec<String>,
    #[serde(default)]
    pub(in crate::runtime::youki) args: Vec<String>,
    #[serde(default)]
    pub(in crate::runtime::youki) env: Vec<SandboxEnv>,
    #[serde(default)]
    pub(in crate::runtime::youki) working_dir: String,
    #[serde(default)]
    pub(in crate::runtime::youki) volume_mounts: Vec<SandboxMount>,
}
