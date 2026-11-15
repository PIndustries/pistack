// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::document::{fill_projections, PodSandbox};
use crate::controllers::pod::slab::{SlabBind, MOUNTED_VOLUMES};
use crate::types::{Pod, MACHINE_POD_LABEL};
use forge::{Machine, MachineKind, MachineSpec};
use pistack_store::StoreError;
impl PodController {
    pub(in crate::controllers::pod) fn create_machine(
        &self,
        pod: &Pod,
        networks: Vec<String>,
        bind: Option<&SlabBind>,
    ) -> Result<Machine, StoreError> {
        let want = pod.spec.resources();
        let size = self.size_for(&want)?;
        let image = pod
            .spec
            .containers
            .first()
            .map(|c| c.image.clone())
            .unwrap_or_default();
        let mut sandbox = PodSandbox::from_pod(pod);
        sandbox.networks = networks.clone();
        fill_projections(&self.store, &mut sandbox);
        if let Some(bind) = bind {
            sandbox.volume_paths = bind.paths.clone();
            sandbox.volume_devices = bind.devices.clone();
        }
        let user_data = serde_json::to_string(&sandbox)?;
        let volumes = pod
            .spec
            .volumes
            .iter()
            .filter_map(|v| match &v.source {
                crate::types::VolumeSource::Volume(id) => Some(id.to_string()),
                _ => None,
            })
            .collect();
        let spec = MachineSpec {
            kind: MachineKind::Container,
            size,
            image,
            networks,
            volumes,
            user_data: Some(user_data),
            host_selector: pod.spec.node_selector.clone(),
            ..Default::default()
        };
        let mut m = forge::new_machine(
            format!("pod-{}", pod.meta.name),
            pod.meta.project.clone(),
            spec,
        );
        m.meta.labels = pod.meta.labels.clone();
        m.meta
            .labels
            .insert(MACHINE_POD_LABEL.into(), pod.meta.id.to_string());
        if let Some(bind) = bind {
            m.meta
                .annotations
                .insert(forge::NODE_PIN_ANNOTATION.into(), bind.node.to_string());
            m.meta
                .annotations
                .insert(MOUNTED_VOLUMES.into(), bind.mounted.clone());
        }
        m.meta.description = format!("pod {}", pod.meta.name);
        match self.machines().create(m) {
            Ok(m) => Ok(m),
            Err(StoreError::Conflict(_)) => {
                // A previous pass created it but the status write was lost.
                let existing = self
                    .machines()
                    .get_by_name(pod.meta.project.as_ref(), &format!("pod-{}", pod.meta.name))?;
                if existing.meta.labels.get(MACHINE_POD_LABEL) == Some(&pod.meta.id.to_string()) {
                    Ok(existing)
                } else {
                    Err(StoreError::Conflict(format!(
                        "machine 'pod-{}' exists and belongs to another pod",
                        pod.meta.name
                    )))
                }
            }
            Err(e) => Err(e),
        }
    }
}
