// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::{SlabVolSpec, SlabVolStatus};
use pistack_types::Resource;
pub(in crate::controllers::pod) type SlabVolume = Resource<SlabVolSpec, SlabVolStatus>;
