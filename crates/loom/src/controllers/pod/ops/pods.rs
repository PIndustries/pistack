// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::types::Pod;
use pistack_store::Collection;
impl PodController {
    pub(in crate::controllers::pod) fn pods(&self) -> Collection<Pod> {
        self.store.collection::<Pod>()
    }
}
