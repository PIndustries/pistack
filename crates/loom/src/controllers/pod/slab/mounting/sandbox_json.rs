// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::document::{fill_projections, PodSandbox};
use crate::controllers::pod::slab::SlabBind;
use crate::types::Pod;
use pistack_store::StoreError;
impl PodController {
    pub(in crate::controllers::pod) fn sandbox_json(
        &self,
        pod: &Pod,
        networks: &[String],
        bind: &SlabBind,
    ) -> Result<String, StoreError> {
        let mut sandbox = PodSandbox::from_pod(pod);
        sandbox.networks = networks.to_vec();
        fill_projections(&self.store, &mut sandbox);
        sandbox.volume_paths = bind.paths.clone();
        sandbox.volume_devices = bind.devices.clone();
        Ok(serde_json::to_string(&sandbox)?)
    }
}
