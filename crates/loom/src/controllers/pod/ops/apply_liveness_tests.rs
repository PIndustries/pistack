// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::*;
use crate::controllers::pod::probes::test_support::*;
use crate::types::{PodPhase, RestartPolicy, POD_READY};
use forge::Machine;
use pistack_rpc::proto::PROBE_LIVENESS;

#[tokio::test]
pub(in crate::controllers::pod) async fn liveness_failure_restarts_only_that_container() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![
            ctr("web", Some(http_probe(8080, 1, 3)), None),
            ctr("sidecar", None, None),
        ],
        vec![streak("web", PROBE_LIVENESS, 0, 3, "http status 500")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Pending);
    assert!(!pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "Restarting");
    assert_eq!(reason(&pod, LIVENESS_CONDITION), "ProbeFailed");
    assert!(pod.status.message.contains("web"));
    let web = pod
        .status
        .container_statuses
        .iter()
        .find(|c| c.name == "web")
        .unwrap();
    let side = pod
        .status
        .container_statuses
        .iter()
        .find(|c| c.name == "sidecar")
        .unwrap();
    assert_eq!(web.restart_count, 1);
    assert_eq!(side.restart_count, 0);
    assert!(!web.ready);
    assert!(!side.ready);
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 1);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn liveness_below_threshold_does_not_restart() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", Some(http_probe(8080, 1, 3)), None)],
        vec![streak("web", PROBE_LIVENESS, 0, 2, "connect: refused")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(pod.status.is_ready());
    assert_eq!(pod.status.container_statuses[0].restart_count, 0);
    assert_eq!(reason(&pod, LIVENESS_CONDITION), "Ok");
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 0);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn liveness_failure_restarts_on_failure_policy() {
    let fx = running_pod(
        RestartPolicy::OnFailure,
        vec![ctr("web", Some(http_probe(8080, 1, 1)), None)],
        vec![streak("web", PROBE_LIVENESS, 0, 1, "timed out")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Pending);
    assert_eq!(pod.status.container_statuses[0].restart_count, 1);
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 1);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn liveness_failure_does_not_restart_when_policy_is_never() {
    let fx = running_pod(
        RestartPolicy::Never,
        vec![ctr("web", Some(http_probe(8080, 1, 3)), None)],
        vec![streak("web", PROBE_LIVENESS, 0, 3, "http status 500")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert_eq!(pod.status.container_statuses[0].restart_count, 0);
    assert!(pod.status.container_statuses[0].ready);
    assert_eq!(reason(&pod, LIVENESS_CONDITION), "ProbeFailed");
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 0);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn exec_liveness_failure_restarts_at_the_threshold() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", Some(exec_probe_with(1, 3)), None)],
        vec![streak("web", PROBE_LIVENESS, 0, 3, "exit 1")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Pending);
    assert_eq!(pod.status.container_statuses[0].restart_count, 1);
    assert_eq!(reason(&pod, LIVENESS_CONDITION), "ProbeFailed");
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 1);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn exec_liveness_below_threshold_does_not_restart() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", Some(exec_probe_with(1, 3)), None)],
        vec![streak("web", PROBE_LIVENESS, 0, 2, "timed out")],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert_eq!(pod.status.container_statuses[0].restart_count, 0);
    assert_eq!(reason(&pod, LIVENESS_CONDITION), "Ok");
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 0);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn exec_liveness_is_unsupported_and_does_not_restart() {
    let mut report = unsupported_report("web", PROBE_LIVENESS);
    report.failures = 9;
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", Some(exec_probe()), None)],
        vec![report],
        0,
        0,
        &[],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert_eq!(pod.status.container_statuses[0].restart_count, 0);
    assert_eq!(reason(&pod, LIVENESS_CONDITION), "Unsupported");
    let message = pod
        .status
        .conditions
        .iter()
        .find(|c| c.kind == LIVENESS_CONDITION)
        .unwrap();
    assert!(message.message.contains("not supported"));
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 0);
}
#[tokio::test]
pub(in crate::controllers::pod) async fn liveness_restart_in_flight_is_not_counted_twice() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", Some(http_probe(8080, 1, 3)), None)],
        vec![streak("web", PROBE_LIVENESS, 0, 9, "http status 500")],
        1,
        0,
        &[("web", 1)],
    );
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Pending);
    assert_eq!(pod.status.container_statuses[0].restart_count, 1);
    let machine = fx.store.collection::<Machine>().get(&fx.machine).unwrap();
    assert_eq!(machine.spec.restarts, 1);
    assert_eq!(machine.status.restarts, 0);
}
