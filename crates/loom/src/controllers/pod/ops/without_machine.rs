// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::patch_status;
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::networks::pick_networks;
use crate::controllers::pod::probes::ensure_container_statuses;
use crate::controllers::pod::slab::SlabHold;
use crate::types::{set_condition, ContainerState, Pod, PodPhase, RestartPolicy, POD_READY};
use forge::Machine;
impl PodController {
    pub(in crate::controllers::pod) async fn without_machine(
        &self,
        pod: &Pod,
        machines: &[Machine],
    ) -> anyhow::Result<()> {
        let vanished = pod.status.machine.is_some();
        let recreate = match pod.spec.restart_policy {
            RestartPolicy::Always => true,
            RestartPolicy::OnFailure => pod.status.phase != PodPhase::Succeeded,
            RestartPolicy::Never => !vanished && !pod.status.phase.is_terminal(),
        };
        if vanished && !recreate {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                if !s.phase.is_terminal() {
                    s.phase = PodPhase::Failed;
                    s.message = "machine disappeared".into();
                }
                s.pod_ip = None;
                s.host = None;
                for c in &mut s.container_statuses {
                    c.ready = false;
                }
                set_condition(&mut s.conditions, POD_READY, false, "MachineGone", "");
            })?;
            return Ok(());
        }
        if !recreate {
            return Ok(());
        }
        let bind = match self.slab_hold(pod, None, machines)? {
            SlabHold::Wait(message) => {
                self.mark_waiting(pod, &message)?;
                return Ok(());
            }
            SlabHold::None => None,
            SlabHold::Ready(bind) => Some(bind),
        };
        let networks = match pick_networks(&self.store, pod) {
            Ok(networks) => networks,
            Err(message) => {
                if pod.status.message != message {
                    patch_status(&self.pods(), &pod.meta.id, |s| {
                        s.phase = PodPhase::Pending;
                        s.message = message.clone();
                        set_condition(
                            &mut s.conditions,
                            "PodScheduled",
                            false,
                            "NoNetwork",
                            &message,
                        );
                    })?;
                    tracing::warn!(pod = %pod.meta.id, %message, "loom: pod has no network");
                }
                return Ok(());
            }
        };
        let m = self.create_machine(pod, networks, bind.as_ref())?;
        let mid = m.meta.id.clone();
        let names: Vec<(String, String)> = pod
            .spec
            .containers
            .iter()
            .map(|c| (c.name.clone(), c.image.clone()))
            .collect();
        patch_status(&self.pods(), &pod.meta.id, |s| {
            s.machine = Some(mid.clone());
            s.phase = PodPhase::Pending;
            s.pod_ip = None;
            s.host = None;
            s.message.clear();
            ensure_container_statuses(s, &names);
            for c in &mut s.container_statuses {
                if vanished {
                    c.restart_count += 1;
                }
                c.ready = false;
                c.state = ContainerState::Waiting {
                    reason: "ContainerCreating".into(),
                };
            }
            set_condition(&mut s.conditions, "PodScheduled", false, "Pending", "");
            set_condition(&mut s.conditions, POD_READY, false, "Starting", "");
        })?;
        tracing::info!(pod = %pod.meta.id, machine = %mid, "loom: machine created for pod");
        Ok(())
    }
}
