// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use serde::Deserialize;

#[derive(Deserialize)]
pub(crate) struct RecordDoc {
    #[serde(default)]
    pub(crate) zone: String,
    #[serde(default)]
    pub(crate) name: String,
    #[serde(rename = "type", default)]
    pub(crate) kind: String,
    #[serde(default)]
    pub(crate) ttl: Option<u32>,
    #[serde(default)]
    pub(crate) records: Vec<String>,
}
