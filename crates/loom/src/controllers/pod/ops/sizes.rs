// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use forge::Size;
use pistack_store::Collection;
impl PodController {
    pub(in crate::controllers::pod) fn sizes(&self) -> Collection<Size> {
        self.store.collection::<Size>()
    }
}
