// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(crate) fn read_cfg(bundle: &Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(bundle.join("config.json")).unwrap()).unwrap()
}
