// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::patch_status;
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::networks::networks_of;
use crate::controllers::pod::slab::{machine_live, mounted_set, SlabBind, MOUNTED_VOLUMES};
use crate::types::{set_condition, ContainerState, Pod, PodPhase, POD_READY};
use forge::Machine;
use pistack_store::StoreError;
impl PodController {
    /// Write the attached paths onto the machine. A volume that showed up
    /// after the containers were created bumps the restart counter.
    /// Returns true when the pod was marked not Ready for that restart.
    pub(in crate::controllers::pod) fn sync_mounted(
        &self,
        pod: &Pod,
        machine: &Machine,
        bind: &SlabBind,
    ) -> Result<bool, StoreError> {
        let mounted = mounted_set(machine);
        let missing: Vec<&str> = bind
            .mounted
            .split(',')
            .filter(|id| !id.is_empty() && !mounted.contains(*id))
            .collect();
        let networks = if machine.spec.networks.is_empty() {
            networks_of(pod)
        } else {
            machine.spec.networks.clone()
        };
        let user_data = self.sandbox_json(pod, &networks, bind)?;
        let node = bind.node.to_string();
        if machine_live(machine) && !missing.is_empty() {
            let id = missing[0];
            let name = bind.names.get(id).map(String::as_str).unwrap_or(id);
            let message = format!("volume {name} attached after the pod started");
            match self.machines().modify(&machine.meta.id, |m| {
                m.spec.user_data = Some(user_data.clone());
                m.spec.restarts += 1;
                m.spec.power = forge::DesiredPower::On;
                m.meta
                    .annotations
                    .insert(MOUNTED_VOLUMES.into(), bind.mounted.clone());
                m.meta
                    .annotations
                    .insert(forge::NODE_PIN_ANNOTATION.into(), node.clone());
                m.meta.annotations.remove(forge::WAIT_VOLUME_ANNOTATION);
                Ok(())
            }) {
                Ok(_) | Err(StoreError::NotFound(_)) => {}
                Err(e) => return Err(e),
            }
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.phase = PodPhase::Pending;
                s.message = message.clone();
                for c in &mut s.container_statuses {
                    c.ready = false;
                    c.state = ContainerState::Waiting {
                        reason: "VolumeAttached".into(),
                    };
                }
                set_condition(
                    &mut s.conditions,
                    POD_READY,
                    false,
                    "VolumeAttached",
                    &message,
                );
            })?;
            tracing::info!(pod = %pod.meta.id, %message, "loom: recreating pod for a late slab volume");
            return Ok(true);
        }
        let pin_now = machine
            .meta
            .annotations
            .get(forge::NODE_PIN_ANNOTATION)
            .map(String::as_str);
        let mounted_now = machine
            .meta
            .annotations
            .get(MOUNTED_VOLUMES)
            .map(String::as_str);
        let waiting = machine
            .meta
            .annotations
            .contains_key(forge::WAIT_VOLUME_ANNOTATION);
        let data_now = machine.spec.user_data.as_deref();
        if pin_now == Some(node.as_str())
            && mounted_now == Some(bind.mounted.as_str())
            && !waiting
            && data_now == Some(user_data.as_str())
        {
            return Ok(false);
        }
        match self.machines().modify(&machine.meta.id, |m| {
            m.spec.user_data = Some(user_data.clone());
            m.meta
                .annotations
                .insert(MOUNTED_VOLUMES.into(), bind.mounted.clone());
            m.meta
                .annotations
                .insert(forge::NODE_PIN_ANNOTATION.into(), node.clone());
            let old = m.meta.annotations.remove(forge::WAIT_VOLUME_ANNOTATION);
            if old.as_deref() == Some(m.status.message.as_str()) {
                m.status.message.clear();
            }
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => Ok(false),
            Err(e) => Err(e),
        }
    }
}
#[cfg(test)]
#[path = "sync_mounted_tests.rs"]
mod sync_mounted_tests;
