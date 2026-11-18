// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_rpc::proto::ExecEnv;
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(crate) struct ExecBody {
    pub(crate) container: String,
    /// Argv. This is not a shell line.
    pub(crate) command: Vec<String>,
    #[serde(default)]
    pub(crate) timeout_secs: Option<u64>,
    #[serde(default)]
    pub(crate) env: Vec<ExecEnv>,
    #[serde(default)]
    pub(crate) cwd: Option<String>,
}
