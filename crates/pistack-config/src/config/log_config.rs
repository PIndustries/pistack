// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! `LogConfig`: the `[log]` section.

use serde::{Deserialize, Serialize};

use super::log_format::LogFormat;

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default, deny_unknown_fields)]
pub struct LogConfig {
    pub level: String,
    pub format: LogFormat,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: "info".into(),
            format: LogFormat::Text,
        }
    }
}
