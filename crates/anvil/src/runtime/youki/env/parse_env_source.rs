// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::EnvSource;

pub(in crate::runtime::youki) fn parse_env_source(value: &serde_json::Value) -> Option<EnvSource> {
    let source: EnvSource = serde_json::from_value(value.clone()).ok()?;
    let (name, key) = match &source {
        EnvSource::ConfigMap { name, key } | EnvSource::Secret { name, key } => (name, key),
    };
    if name.is_empty() || key.is_empty() {
        None
    } else {
        Some(source)
    }
}
