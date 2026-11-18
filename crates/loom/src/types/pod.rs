// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::pod_spec::PodSpec;
use super::pod_status::PodStatus;

pub type Pod = Resource<PodSpec, PodStatus>;
