// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::slab::{decide_slab, slab_refs, SlabHold};
use crate::types::Pod;
use forge::Machine;
use pistack_store::StoreError;
impl PodController {
    pub(in crate::controllers::pod) fn slab_hold(
        &self,
        pod: &Pod,
        machine: Option<&Machine>,
        machines: &[Machine],
    ) -> Result<SlabHold, StoreError> {
        if slab_refs(pod).is_empty() {
            return Ok(SlabHold::None);
        }
        let vols = self.slab_volumes().list(None)?;
        Ok(decide_slab(pod, machine, machines, &vols))
    }
}
