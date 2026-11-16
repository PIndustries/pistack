// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use fabric::overlay::PlanProvider;

pub(in crate::overlay) enum Realised {
    Tenant(u32),
    External(u32),
    Provider(PlanProvider),
}
