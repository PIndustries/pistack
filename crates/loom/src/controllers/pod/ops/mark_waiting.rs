// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::patch_status;
use crate::controllers::pod::controller::PodController;
use crate::types::{set_condition, ContainerState, Pod, PodPhase, POD_READY};
use pistack_store::StoreError;
impl PodController {
    pub(in crate::controllers::pod) fn mark_waiting(
        &self,
        pod: &Pod,
        message: &str,
    ) -> Result<(), StoreError> {
        patch_status(&self.pods(), &pod.meta.id, |s| {
            if !s.phase.is_terminal() {
                s.phase = PodPhase::Pending;
            }
            s.message = message.to_string();
            for c in &mut s.container_statuses {
                c.ready = false;
                c.state = ContainerState::Waiting {
                    reason: "VolumeNotAttached".into(),
                };
            }
            set_condition(
                &mut s.conditions,
                POD_READY,
                false,
                "VolumeNotAttached",
                message,
            );
        })?;
        Ok(())
    }
}
