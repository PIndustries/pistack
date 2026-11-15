// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::*;
use crate::controllers::pod::probes::test_support::*;
use crate::types::{PodPhase, RestartPolicy, POD_READY};

#[tokio::test]
pub(in crate::controllers::pod) async fn pod_without_probes_is_ready_once_running() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", None, None)],
        vec![],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert_eq!(pod.status.pod_ip.as_deref(), Some("10.1.0.8"));
    assert!(pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "Running");
    assert!(pod.status.container_statuses[0].ready);
    assert!(pod
        .status
        .conditions
        .iter()
        .all(|c| c.kind != LIVENESS_CONDITION));
}
