// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::Resource;

use super::volume_spec_view::VolumeSpecView;
use super::volume_status_view::VolumeStatusView;

pub(crate) type Volume = Resource<VolumeSpecView, VolumeStatusView>;
