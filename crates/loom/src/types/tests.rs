// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

// ---- kinds and well-known keys ------------------------------------------------

use super::*;

#[test]
fn container_without_probes_still_decodes() {
    let container: Container = serde_json::from_str(r#"{"name":"web","image":"img"}"#).unwrap();
    assert!(container.liveness_probe.is_none());
    assert!(container.readiness_probe.is_none());
    let encoded = serde_json::to_value(&container).unwrap();
    assert!(encoded.get("liveness_probe").is_none());
    assert!(encoded.get("readiness_probe").is_none());
}

#[test]
fn probe_defaults_fill_omitted_fields_and_zero_means_default() {
    let probe: Probe =
        serde_json::from_str(r#"{"http_get":{"path":"/health","port":8080}}"#).unwrap();
    assert_eq!(probe.initial_delay_seconds, 0);
    assert_eq!(probe.period_seconds, 10);
    assert_eq!(probe.timeout_seconds, 1);
    assert_eq!(probe.success_threshold, 1);
    assert_eq!(probe.failure_threshold, 3);
    assert_eq!(probe.http_get.as_ref().unwrap().path, "/health");
    assert_eq!(probe.handler(), ProbeHandlerKind::Http);

    let tcp: Probe = serde_json::from_str(r#"{"tcp_socket":{"port":80}}"#).unwrap();
    assert_eq!(tcp.handler(), ProbeHandlerKind::Tcp);
    assert_eq!(tcp.period(), 10);

    let zero = Probe {
        period_seconds: 0,
        timeout_seconds: 0,
        success_threshold: 0,
        failure_threshold: 0,
        ..Probe::default()
    };
    assert_eq!(zero.period(), 10);
    assert_eq!(zero.timeout(), 1);
    assert_eq!(zero.successes_required(), 1);
    assert_eq!(zero.failures_required(), 3);

    let exec: Probe = serde_json::from_str(r#"{"exec":{"command":["/bin/true"]}}"#).unwrap();
    assert_eq!(exec.handler(), ProbeHandlerKind::Exec);
    assert_eq!(exec.exec.unwrap().command, vec!["/bin/true".to_string()]);

    let scheme: HttpGetProbe =
        serde_json::from_str(r#"{"path":"/","port":443,"scheme":"https"}"#).unwrap();
    assert_eq!(scheme.scheme, Some(ProbeScheme::Https));
}

#[test]
fn is_ready_reads_the_ready_condition() {
    let mut status = PodStatus::default();
    assert!(!status.is_ready());
    set_condition(&mut status.conditions, POD_READY, false, "Starting", "");
    assert!(!status.is_ready());
    set_condition(&mut status.conditions, POD_READY, true, "Running", "");
    assert!(status.is_ready());
    set_condition(
        &mut status.conditions,
        "PodScheduled",
        true,
        "Scheduled",
        "",
    );
    assert!(status.is_ready());
}
