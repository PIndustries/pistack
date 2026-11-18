// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::autoscaler_spec::AutoscalerSpec;
use super::autoscaler_status::AutoscalerStatus;

pub type Autoscaler = Resource<AutoscalerSpec, AutoscalerStatus>;
