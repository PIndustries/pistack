// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::lookup_env;
use crate::runtime::youki::env::parse_env_source;
use crate::runtime::youki::spec::Projected;
use crate::runtime::youki::spec::SandboxEnv;
use std::collections::BTreeMap;

pub(in crate::runtime::youki) fn resolve_env_pairs(
    env: &[SandboxEnv],
    config_maps: &BTreeMap<String, Projected>,
    secrets: &BTreeMap<String, Projected>,
) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for entry in env {
        if entry.name.is_empty() {
            continue;
        }
        // A present value_from replaces the literal, even when the ref is unusable.
        if let Some(value) = entry.value_from.as_ref() {
            let Some(source) = parse_env_source(value) else {
                continue;
            };
            let Some(resolved) = lookup_env(&source, config_maps, secrets) else {
                continue;
            };
            out.push((entry.name.clone(), resolved));
        } else {
            out.push((entry.name.clone(), entry.value.clone()));
        }
    }
    out
}
