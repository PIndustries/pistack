// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Schema version written into [`OverlayPlan::version`].
pub const PLAN_VERSION: u32 = 1;

/// Linux VXLAN destination port.
pub const VXLAN_PORT: u16 = 4789;

pub(in crate::overlay) const MAX_VNI: u32 = 16_777_215;
