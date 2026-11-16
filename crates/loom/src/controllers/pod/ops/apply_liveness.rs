// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::patch_status;
use crate::controllers::pod::controller::PodController;
use crate::controllers::pod::document::LIVENESS_CONDITION;
use crate::controllers::pod::probes::{probe_report, unsupported_text};
use crate::types::{
    set_condition, ContainerState, Pod, PodPhase, ProbeHandlerKind, RestartPolicy, POD_READY,
};
use pistack_rpc::proto::{ProbeReport, PROBE_LIVENESS};
use pistack_store::StoreError;
use pistack_types::Id;
impl PodController {
    /// Restart containers whose liveness streak has reached the threshold.
    ///
    /// The whole sandbox restarts, because that is the machine restart
    /// knob, and only the containers that failed have their restart count
    /// bumped. `Never` records the failure and leaves the pod running.
    /// A probe the runtime reports unsupported does not count as a failure.
    pub(in crate::controllers::pod) async fn apply_liveness(
        &self,
        pod: &Pod,
        machine: &Id,
        reports: &[ProbeReport],
    ) -> anyhow::Result<()> {
        let mut failed = Vec::new();
        let mut saw_check = false;
        let mut unsupported_message: Option<String> = None;
        let mut missing = false;
        for container in &pod.spec.containers {
            let Some(probe) = &container.liveness_probe else {
                continue;
            };
            match probe.handler() {
                ProbeHandlerKind::Http | ProbeHandlerKind::Tcp | ProbeHandlerKind::Exec => {
                    let report = probe_report(reports, &container.name, PROBE_LIVENESS);
                    if report.is_some_and(|r| r.unsupported) {
                        if unsupported_message.is_none() {
                            unsupported_message = Some(unsupported_text(report));
                        }
                        continue;
                    }
                    saw_check = true;
                    let failures = report.map(|r| r.failures).unwrap_or(0);
                    if failures >= probe.failures_required() {
                        failed.push(container.name.clone());
                    }
                }
                ProbeHandlerKind::Missing => missing = true,
            }
        }
        if failed.is_empty() && !saw_check && unsupported_message.is_none() && !missing {
            return Ok(());
        }
        if !failed.is_empty() {
            let names = failed.join(", ");
            match pod.spec.restart_policy {
                RestartPolicy::Always | RestartPolicy::OnFailure => {
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
                        s.message = format!("restarting {names}");
                        for c in &mut s.container_statuses {
                            if failed.contains(&c.name) {
                                c.restart_count += 1;
                            }
                            c.ready = false;
                            c.state = ContainerState::Waiting {
                                reason: "Restarting".into(),
                            };
                        }
                        set_condition(&mut s.conditions, POD_READY, false, "Restarting", "");
                        set_condition(
                            &mut s.conditions,
                            LIVENESS_CONDITION,
                            false,
                            "ProbeFailed",
                            &names,
                        );
                    })?;
                    tracing::info!(pod = %pod.meta.id, containers = %names, "loom: liveness restart");
                }
                RestartPolicy::Never => {
                    patch_status(&self.pods(), &pod.meta.id, |s| {
                        set_condition(
                            &mut s.conditions,
                            LIVENESS_CONDITION,
                            false,
                            "ProbeFailed",
                            &names,
                        );
                    })?;
                }
            }
            return Ok(());
        }
        if saw_check {
            patch_status(&self.pods(), &pod.meta.id, |s| {
                set_condition(&mut s.conditions, LIVENESS_CONDITION, true, "Ok", "");
            })?;
            return Ok(());
        }
        let message = unsupported_message.unwrap_or_else(|| "probe has no handler".to_owned());
        patch_status(&self.pods(), &pod.meta.id, |s| {
            set_condition(
                &mut s.conditions,
                LIVENESS_CONDITION,
                false,
                "Unsupported",
                &message,
            );
        })?;
        Ok(())
    }
}
#[cfg(test)]
#[path = "apply_liveness_tests.rs"]
mod apply_liveness_tests;
