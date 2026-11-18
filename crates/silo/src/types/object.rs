// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use super::object_spec::ObjectSpec;
use super::object_status::ObjectStatus;

pub type Object = Resource<ObjectSpec, ObjectStatus>;
