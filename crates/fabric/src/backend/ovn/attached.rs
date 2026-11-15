// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::Leadership;

pub(in crate::backend::ovn) struct Attached {
    pub(in crate::backend::ovn) leadership: Leadership,
    pub(in crate::backend::ovn) store: Store,
}
