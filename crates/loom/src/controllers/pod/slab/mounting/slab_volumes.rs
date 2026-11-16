// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::slab::SlabVolume;
use pistack_store::Collection;
impl PodController {
    pub(in crate::controllers::pod) fn slab_volumes(&self) -> Collection<SlabVolume> {
        self.store.collection::<SlabVolume>()
    }
}
