// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::probes::test_support::*;
use crate::types::{PodPhase, RestartPolicy, POD_READY};
use pistack_rpc::proto::PROBE_READINESS;

#[tokio::test]
pub(in crate::controllers::pod) async fn exec_readiness_stays_not_ready_when_unsupported() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", None, Some(exec_probe()))],
        vec![unsupported_report("web", PROBE_READINESS)],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(!pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "ContainersNotReady");
    let message = &pod
        .status
        .conditions
        .iter()
        .find(|c| c.kind == POD_READY)
        .unwrap()
        .message;
    assert!(message.contains("not supported"), "{message}");
    assert!(!pod.status.container_statuses[0].ready);
}
