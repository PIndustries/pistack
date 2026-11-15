// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::probes::test_support::*;
use crate::types::{PodPhase, RestartPolicy, POD_READY};
use forge::Machine;

#[tokio::test]
pub(in crate::controllers::pod) async fn rootless_network_gap_hides_the_address_and_is_not_ready() {
    let fx = running_pod(
        RestartPolicy::Always,
        vec![ctr("web", None, None)],
        vec![],
        0,
        0,
        &[],
    );
    let message = "pod has no network: a rootless agent cannot attach the recorded address. Run the agent as root or set PISTACK_YOUKI_SUDO=1";
    fx.store
        .collection::<Machine>()
        .modify(&fx.machine, |m| {
            m.status.message = message.to_owned();
            m.status
                .set_condition("Network", false, "Rootless", message);
            Ok(())
        })
        .unwrap();
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert!(pod.status.pod_ip.is_none());
    assert!(
        pod.status.message.contains("pod has no network"),
        "{}",
        pod.status.message
    );
    let network = pod
        .status
        .conditions
        .iter()
        .find(|c| c.kind == "Network")
        .unwrap();
    assert!(!network.status);
    assert_eq!(network.reason, "Rootless");
    assert!(!pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "NoNetwork");

    fx.store
        .collection::<Machine>()
        .modify(&fx.machine, |m| {
            m.status.message.clear();
            m.status.conditions.retain(|c| c.kind != "Network");
            Ok(())
        })
        .unwrap();
    pass(&fx.store).await;
    let pod = load(&fx.store, &fx.pod);
    assert_eq!(pod.status.phase, PodPhase::Running);
    assert_eq!(pod.status.pod_ip.as_deref(), Some("10.1.0.8"));
    assert!(pod.status.message.is_empty());
    assert!(pod.status.conditions.iter().all(|c| c.kind != "Network"));
    assert!(pod.status.is_ready());
    assert_eq!(reason(&pod, POD_READY), "Running");
}
