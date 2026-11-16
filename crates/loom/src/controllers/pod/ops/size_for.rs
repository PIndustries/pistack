// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::networks::exact_size_name;
use pistack_store::StoreError;
use pistack_types::Resources;
impl PodController {
    /// Smallest existing size the pod fits, else create `pod-<v>x<m>`.
    pub(in crate::controllers::pod) fn size_for(
        &self,
        want: &Resources,
    ) -> Result<String, StoreError> {
        let mut sizes = self.sizes().list(None)?;
        sizes.sort_by_key(|s| {
            let r = s.spec.resources;
            (r.memory_mib, r.vcpus, r.disk_gib, r.gpus)
        });
        if let Some(s) = sizes.iter().find(|s| want.fits_in(&s.spec.resources)) {
            return Ok(s.meta.name.clone());
        }
        let name = exact_size_name(want);
        let exact = Resources {
            vcpus: want.vcpus.max(1),
            memory_mib: want.memory_mib.max(1),
            disk_gib: want.disk_gib,
            gpus: want.gpus,
        };
        match self.sizes().create(forge::new_size(name.clone(), exact)) {
            Ok(_) | Err(StoreError::Conflict(_)) => Ok(name),
            Err(e) => Err(e),
        }
    }
}
