// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::env::lookup_projected;
use crate::runtime::youki::env::EnvSource;
use crate::runtime::youki::spec::Projected;
use std::collections::BTreeMap;

pub(in crate::runtime::youki) fn lookup_env(
    source: &EnvSource,
    config_maps: &BTreeMap<String, Projected>,
    secrets: &BTreeMap<String, Projected>,
) -> Option<String> {
    let (projected, key) = match source {
        EnvSource::ConfigMap { name, key } => (config_maps.get(name)?, key),
        EnvSource::Secret { name, key } => (secrets.get(name)?, key),
    };
    lookup_projected(projected, key)
}
