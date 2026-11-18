// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{Kind, Meta, Resource};
use serde::{Deserialize, Serialize};
use std::ops::{Deref, DerefMut};

use super::size_kind::SIZE_KIND;
use super::size_spec::SizeSpec;
use super::size_status::SizeStatus;

resource_kind!(
    /// A flavor: named bundle of resources.
    Size,
    SizeSpec,
    SizeStatus,
    SIZE_KIND
);
