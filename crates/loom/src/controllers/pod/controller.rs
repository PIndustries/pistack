// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_store::Store;
pub struct PodController {
    pub(in crate::controllers::pod) store: Store,
}
