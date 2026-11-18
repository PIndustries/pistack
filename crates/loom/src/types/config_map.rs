// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::config_map_spec::ConfigMapSpec;
use super::config_map_status::ConfigMapStatus;

pub type ConfigMap = Resource<ConfigMapSpec, ConfigMapStatus>;
