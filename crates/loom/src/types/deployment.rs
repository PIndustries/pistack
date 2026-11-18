// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::deployment_spec::DeploymentSpec;
use super::deployment_status::DeploymentStatus;

pub type Deployment = Resource<DeploymentSpec, DeploymentStatus>;
