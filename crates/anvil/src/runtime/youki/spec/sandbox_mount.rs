// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub struct SandboxMount {
    #[serde(default)]
    pub(in crate::runtime::youki) name: String,
    #[serde(default)]
    pub(in crate::runtime::youki) mount_path: String,
    #[serde(default)]
    pub(in crate::runtime::youki) read_only: bool,
    #[serde(default)]
    pub(in crate::runtime::youki) sub_path: String,
}
