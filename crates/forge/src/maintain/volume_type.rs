// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use super::no_status::NoStatus;
use super::volume_type_spec_view::VolumeTypeSpecView;

pub(crate) type VolumeType = Resource<VolumeTypeSpecView, NoStatus>;
