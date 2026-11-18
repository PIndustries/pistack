// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// `true` on a floating IP this pass owns. `false` on a machine or a
/// project keeps automatic floating IPs away from it.
pub const AUTO_FIP_LABEL: &str = "pistack.io/auto-fip";
