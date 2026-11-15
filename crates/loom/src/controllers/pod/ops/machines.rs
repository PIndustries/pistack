// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use forge::Machine;
use pistack_store::Collection;
impl PodController {
    pub(in crate::controllers::pod) fn machines(&self) -> Collection<Machine> {
        self.store.collection::<Machine>()
    }
}
