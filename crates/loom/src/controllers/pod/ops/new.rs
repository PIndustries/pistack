// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use pistack_store::Store;
impl PodController {
    pub fn new(store: Store) -> Self {
        Self { store }
    }
}
