// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Meta, Resource, Resources};

use super::size::Size;
use super::size_kind::SIZE_KIND;
use super::size_spec::SizeSpec;
use super::size_status::SizeStatus;

/// Sizes are cloud-scoped (project `None`).
pub fn new_size(name: impl Into<String>, resources: Resources) -> Size {
    Size(Resource {
        meta: Meta::new(SIZE_KIND, name, None),
        spec: SizeSpec { resources },
        status: SizeStatus {},
    })
}
