// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::kind;

/// Events that may change a machine's name or addresses.
pub(crate) const TRIGGERS: &[&str] = &[kind::MACHINE, kind::FLOATING_IP, kind::PORT, kind::ZONE];
