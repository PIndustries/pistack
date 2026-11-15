// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::patch_status;
use crate::controllers::pod::controller::PodController;
use crate::types::{set_condition, ContainerState, Pod, PodPhase, POD_READY};
use chrono::{Duration, Utc};
use pistack_store::StoreError;
use pistack_types::Id;
impl PodController {
    pub(in crate::controllers::pod) async fn maybe_restart(
        &self,
        pod: &Pod,
        machine: &Id,
    ) -> anyhow::Result<()> {
        let restarts = pod
            .status
            .container_statuses
            .iter()
            .map(|c| c.restart_count)
            .max()
            .unwrap_or(0);
        let backoff = Duration::seconds((10i64 << restarts.min(5)).min(300));
        let since = pod
            .status
            .conditions
            .iter()
            .find(|c| c.kind == POD_READY)
            .map(|c| c.last_transition)
            .unwrap_or_else(Utc::now);
        if Utc::now() - since < backoff {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                set_condition(
                    &mut s.conditions,
                    "BackOff",
                    true,
                    "Waiting",
                    &format!("restart {} after {}s", restarts + 1, backoff.num_seconds()),
                );
            })?;
            return Ok(());
        }
        match self.machines().modify(machine, |m| {
            m.spec.restarts += 1;
            m.spec.power = forge::DesiredPower::On;
            Ok(())
        }) {
            Ok(_) | Err(StoreError::NotFound(_)) => {}
            Err(e) => return Err(e.into()),
        }
        patch_status(&self.pods(), &pod.meta.id, |s| {
            s.phase = PodPhase::Pending;
            s.message = "restarting".into();
            for c in &mut s.container_statuses {
                c.restart_count += 1;
                c.ready = false;
                c.state = ContainerState::Waiting {
                    reason: "Restarting".into(),
                };
            }
            set_condition(&mut s.conditions, POD_READY, false, "Restarting", "");
            set_condition(&mut s.conditions, "BackOff", false, "Restarted", "");
        })?;
        tracing::info!(pod = %pod.meta.id, restarts = restarts + 1, "loom: restarting pod");
        Ok(())
    }
}
