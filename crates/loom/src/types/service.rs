// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::service_spec::ServiceSpec;
use super::service_status::ServiceStatus;

pub type Service = Resource<ServiceSpec, ServiceStatus>;
