// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

pub(in crate::runtime::youki) fn docker_manifest(staging: &Path) -> bool {
    let Ok(bytes) = std::fs::read(staging.join("manifest.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return false;
    };
    value.as_array().and_then(|a| a.first()).is_some_and(|e| {
        e.get("Layers").is_some() || e.get("layers").is_some() || e.get("Config").is_some()
    })
}
