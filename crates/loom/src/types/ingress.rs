// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

// ---- kinds and well-known keys ------------------------------------------------

use super::ingress_spec::IngressSpec;
use super::ingress_status::IngressStatus;

pub type Ingress = Resource<IngressSpec, IngressStatus>;
