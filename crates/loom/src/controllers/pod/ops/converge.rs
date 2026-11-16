// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::patch_status;
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::document::EXITED_CONDITION;
use crate::controllers::pod::probes::{apply_readiness, ensure_container_statuses};
use crate::controllers::pod::slab::SlabHold;
use crate::types::{set_condition, ContainerState, Pod, PodPhase, RestartPolicy, POD_READY};
use chrono::Utc;
use forge::{Machine, PowerState};
use pistack_store::StoreError;
use pistack_types::Phase;
impl PodController {
    pub(in crate::controllers::pod) async fn converge(
        &self,
        pod: &Pod,
        machines: &[Machine],
    ) -> anyhow::Result<()> {
        let Some(m) = self.machine_of(pod, machines) else {
            return self.without_machine(pod, machines).await;
        };
        if forge::delete_requested(m) {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                s.message = "machine is being deleted".into();
            })?;
            return Ok(());
        }
        match self.slab_hold(pod, Some(m), machines)? {
            SlabHold::Wait(message) => {
                self.hold_for_volume(pod, m, &message)?;
                return Ok(());
            }
            SlabHold::Ready(bind) => {
                if self.sync_mounted(pod, m, &bind)? {
                    return Ok(());
                }
            }
            SlabHold::None => {}
        }
        let mid_now = m.meta.id.clone();
        let m = match self.machines().get(&mid_now) {
            Ok(m) => m,
            Err(StoreError::NotFound(_)) => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let host = m.status.host.clone();
        let addr = m.status.addresses.first().cloned();
        let mid = m.meta.id.clone();
        let exited = m
            .status
            .conditions
            .iter()
            .find(|c| c.kind == EXITED_CONDITION && c.status)
            .map(|c| (c.reason.parse::<i32>().unwrap_or(-1), c.message.clone()));
        let mphase = m.status.phase;
        let mpower = m.status.power;
        // A reboot we asked for that forge has not carried out yet: the
        // machine still looks exited, but the pod is on its way back up.
        let restart_in_flight = m.spec.restarts > m.status.restarts;
        let mmsg = m.status.message.clone();
        let network_gap = m
            .status
            .conditions
            .iter()
            .find(|c| c.kind == "Network" && !c.status)
            .map(|c| {
                if c.message.is_empty() {
                    mmsg.clone()
                } else {
                    c.message.clone()
                }
            });
        let names: Vec<(String, String)> = pod
            .spec
            .containers
            .iter()
            .map(|c| (c.name.clone(), c.image.clone()))
            .collect();
        let reports = m.status.probes.clone();
        // Same branch as the running arm below: power is on (or not yet
        // known) and a restart we asked for has not landed.
        let check_liveness =
            matches!(mphase, Phase::Ready) && !restart_in_flight && mpower != PowerState::Off;

        let updated = patch_status(&self.pods(), &pod.meta.id, |s| {
            s.machine = Some(mid.clone());
            s.host = host.clone();
            ensure_container_statuses(s, &names);
            set_condition(
                &mut s.conditions,
                "PodScheduled",
                host.is_some(),
                if host.is_some() {
                    "Scheduled"
                } else {
                    "Unschedulable"
                },
                if host.is_some() { "" } else { &mmsg },
            );
            match mphase {
                Phase::Ready if restart_in_flight => {
                    s.phase = PodPhase::Pending;
                    s.message = "restarting".into();
                }
                Phase::Ready => match (mpower, &exited) {
                    (PowerState::Off, Some((code, which))) => {
                        let ok = *code == 0;
                        s.phase = if ok {
                            PodPhase::Succeeded
                        } else {
                            PodPhase::Failed
                        };
                        s.message = format!("container {which} exited with code {code}");
                        for c in &mut s.container_statuses {
                            c.ready = false;
                            c.state = ContainerState::Terminated {
                                exit_code: *code,
                                reason: if ok { "Completed" } else { "Error" }.into(),
                                finished_at: Utc::now(),
                            };
                        }
                        set_condition(&mut s.conditions, POD_READY, false, "Terminated", "");
                    }
                    (PowerState::Off, None) => {
                        s.phase = PodPhase::Unknown;
                        s.message = "machine is powered off".into();
                        set_condition(&mut s.conditions, POD_READY, false, "PoweredOff", "");
                    }
                    _ => {
                        s.phase = PodPhase::Running;
                        let started = s.start_time.unwrap_or_else(Utc::now);
                        s.start_time = Some(started);
                        if let Some(gap) = network_gap.as_deref() {
                            s.pod_ip = None;
                            s.message = gap.to_owned();
                            set_condition(&mut s.conditions, "Network", false, "Rootless", gap);
                            apply_readiness(pod, &reports, s);
                            set_condition(&mut s.conditions, POD_READY, false, "NoNetwork", gap);
                        } else {
                            s.pod_ip = addr.clone();
                            s.message.clear();
                            s.conditions.retain(|c| c.kind != "Network");
                            apply_readiness(pod, &reports, s);
                        }
                    }
                },
                Phase::Error => {
                    s.phase = PodPhase::Failed;
                    s.message = mmsg.clone();
                    for c in &mut s.container_statuses {
                        c.ready = false;
                        c.state = ContainerState::Waiting {
                            reason: "StartFailed".into(),
                        };
                    }
                    set_condition(&mut s.conditions, POD_READY, false, "MachineError", &mmsg);
                }
                Phase::Pending | Phase::Creating | Phase::Updating => {
                    if !s.phase.is_terminal() || mphase != Phase::Updating {
                        s.phase = PodPhase::Pending;
                    }
                    s.message = mmsg.clone();
                    for c in &mut s.container_statuses {
                        c.ready = false;
                        c.state = ContainerState::Waiting {
                            reason: "ContainerCreating".into(),
                        };
                    }
                    set_condition(&mut s.conditions, POD_READY, false, "Starting", "");
                }
                Phase::Deleting => {
                    s.message = "machine is being deleted".into();
                }
            }
        })?;

        if check_liveness {
            self.apply_liveness(&updated, &mid, &reports).await?;
        }

        // Restart policy: on a terminal pod whose machine still exists, ask
        // forge for a reboot once the backoff has elapsed.
        let restart = matches!(
            (updated.spec.restart_policy, updated.status.phase),
            (
                RestartPolicy::Always,
                PodPhase::Succeeded | PodPhase::Failed
            ) | (RestartPolicy::OnFailure, PodPhase::Failed)
        );
        if restart && !restart_in_flight && matches!(mphase, Phase::Ready | Phase::Error) {
            self.maybe_restart(&updated, &mid).await?;
        }
        Ok(())
    }
}
#[cfg(test)]
#[path = "converge_tests.rs"]
mod converge_tests;
