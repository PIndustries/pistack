// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Same JSON shape as loom's volume source: `"empty_dir"`,
/// `{"config_map":"<name>"}`, `{"secret":"<name>"}`, `{"volume":"<id>"}`.
use serde::Deserialize;
use serde::Serialize;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum VolumeSrc {
    EmptyDir,
    Secret(String),
    ConfigMap(String),
    Volume(String),
}
