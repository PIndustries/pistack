// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

#[cfg(test)]
use crate::runtime::youki::env::resolve_env_pairs;
use crate::runtime::youki::image::ImageConfig;
use crate::runtime::youki::spec::process_spec;
use crate::runtime::youki::spec::Launch;
use crate::runtime::youki::spec::Projected;
use crate::runtime::youki::spec::SandboxEnv;
use std::collections::BTreeMap;

#[cfg(test)]
pub(in crate::runtime::youki) fn resolve_process_env(
    image_env: &[String],
    container_env: &[SandboxEnv],
    config_maps: &BTreeMap<String, Projected>,
    secrets: &BTreeMap<String, Projected>,
) -> Vec<String> {
    let launch = Launch {
        env: resolve_env_pairs(container_env, config_maps, secrets),
        ..Launch::default()
    };
    let cfg = ImageConfig {
        env: image_env.to_vec(),
        ..ImageConfig::default()
    };
    process_spec(Some(&launch), &cfg).1
}
