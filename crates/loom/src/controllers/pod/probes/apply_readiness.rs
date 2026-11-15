// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::probes::{
    hold_not_ready, mark_running, probe_report, unsupported_text,
};
use crate::types::{set_condition, Pod, PodStatus, ProbeHandlerKind, POD_READY};
use pistack_rpc::proto::{ProbeReport, PROBE_READINESS};
/// Set each container's ready flag and the pod Ready condition from the
/// readiness streaks on the machine. A container with no readiness probe
/// is ready. Phase is left to the caller.
pub(in crate::controllers::pod) fn apply_readiness(
    pod: &Pod,
    reports: &[ProbeReport],
    status: &mut PodStatus,
) {
    let mut waiting = Vec::new();
    for cs in &mut status.container_statuses {
        let Some(spec) = pod.spec.containers.iter().find(|c| c.name == cs.name) else {
            continue;
        };
        let Some(probe) = spec.readiness_probe.as_ref() else {
            cs.ready = true;
            mark_running(&mut cs.state);
            continue;
        };
        match probe.handler() {
            ProbeHandlerKind::Http | ProbeHandlerKind::Tcp | ProbeHandlerKind::Exec => {
                let report = probe_report(reports, &cs.name, PROBE_READINESS);
                if report.is_some_and(|r| r.unsupported) {
                    let message = unsupported_text(report);
                    hold_not_ready(cs, &message, &mut waiting);
                    continue;
                }
                let successes = report.map(|r| r.successes).unwrap_or(0);
                if successes >= probe.successes_required() {
                    cs.ready = true;
                    mark_running(&mut cs.state);
                } else {
                    let message = report
                        .map(|r| r.message.as_str())
                        .filter(|m| !m.is_empty())
                        .unwrap_or("readiness probe has not passed");
                    hold_not_ready(cs, message, &mut waiting);
                }
            }
            ProbeHandlerKind::Missing => {
                hold_not_ready(cs, "probe has no handler", &mut waiting);
            }
        }
    }
    if status.container_statuses.iter().all(|c| c.ready) {
        set_condition(&mut status.conditions, POD_READY, true, "Running", "");
    } else {
        set_condition(
            &mut status.conditions,
            POD_READY,
            false,
            "ContainersNotReady",
            &waiting.join("; "),
        );
    }
}
#[cfg(test)]
#[path = "apply_readiness_tests.rs"]
mod apply_readiness_tests;
