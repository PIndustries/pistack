// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::probes::test_support::*;
use crate::types::{PodPhase, RestartPolicy, POD_READY};
use forge::Machine;
use pistack_rpc::proto::PROBE_READINESS;

#[tokio::test]
pub(in crate::controllers::pod) async fn readiness_holds_ready_until_the_success_streak() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", None, Some(http_probe(8080, 2, 3)))],
        vec![streak("web", PROBE_READINESS, 1, 0, "")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(!pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "ContainersNotReady");
    assert!(!pod.status.container_statuses[0].ready);
    let message = pod
        .status
        .conditions
        .iter()
        .find(|c| c.kind == POD_READY)
        .unwrap()
        .message
        .clone();
    assert!(message.contains("readiness probe has not passed"));

    fx.store
        .collection::<Machine>()
        .modify(&fx.machine, |m| {
            m.status.probes = vec![streak("web", PROBE_READINESS, 2, 0, "")];
            Ok(())
        })
        .unwrap();
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "Running");
    assert!(pod.status.container_statuses[0].ready);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn tcp_readiness_passes_once_the_streak_is_met() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", None, Some(tcp_probe(80, 1)))],
        vec![streak("web", PROBE_READINESS, 1, 0, "")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(pod.status.is_ready());
    assert!(pod.status.container_statuses[0].ready);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn exec_readiness_follows_the_success_threshold() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", None, Some(exec_probe_with(2, 3)))],
        vec![streak("web", PROBE_READINESS, 1, 0, "")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(!pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "ContainersNotReady");
    assert!(!pod.status.container_statuses[0].ready);

    fx.store
        .collection::<Machine>()
        .modify(&fx.machine, |m| {
            m.status.probes = vec![streak("web", PROBE_READINESS, 2, 0, "")];
            Ok(())
        })
        .unwrap();
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert!(pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "Running");
    assert!(pod.status.container_statuses[0].ready);
}
