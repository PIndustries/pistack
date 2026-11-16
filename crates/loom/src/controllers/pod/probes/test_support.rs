// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::*;
use crate::controllers::Controller;
use crate::types::{
    Container, ExecProbe, HttpGetProbe, Pod, PodSpec, Probe, RestartPolicy, TcpSocketProbe,
    MACHINE_POD_LABEL,
};
use forge::{new_machine, Machine, MachineKind, MachineSpec, PowerState};
use pistack_rpc::proto::ProbeReport;
use pistack_store::Store;
use pistack_types::{kind, Id, Phase};

pub(in crate::controllers::pod) fn http_probe(port: u16, success: u32, failure: u32) -> Probe {
    Probe {
        http_get: Some(HttpGetProbe {
            path: "/health".into(),
            port,
            host: None,
            scheme: None,
        }),
        success_threshold: success,
        failure_threshold: failure,
        ..Probe::default()
    }
}
pub(in crate::controllers::pod) fn tcp_probe(port: u16, success: u32) -> Probe {
    Probe {
        tcp_socket: Some(TcpSocketProbe { port }),
        success_threshold: success,
        ..Probe::default()
    }
}
pub(in crate::controllers::pod) fn exec_probe() -> Probe {
    exec_probe_with(1, 3)
}
pub(in crate::controllers::pod) fn exec_probe_with(success: u32, failure: u32) -> Probe {
    Probe {
        exec: Some(ExecProbe {
            command: vec!["/bin/true".into()],
        }),
        success_threshold: success,
        failure_threshold: failure,
        ..Probe::default()
    }
}
pub(in crate::controllers::pod) fn ctr(
    name: &str,
    liveness: Option<Probe>,
    readiness: Option<Probe>,
) -> Container {
    Container {
        name: name.into(),
        image: "img".into(),
        liveness_probe: liveness,
        readiness_probe: readiness,
        ..Container::default()
    }
}
pub(in crate::controllers::pod) fn streak(
    container: &str,
    kind: &str,
    successes: u32,
    failures: u32,
    message: &str,
) -> ProbeReport {
    ProbeReport {
        container: container.into(),
        kind: kind.into(),
        success: failures == 0 && successes > 0,
        successes,
        failures,
        message: message.into(),
        unsupported: false,
    }
}
pub(in crate::controllers::pod) fn unsupported_report(container: &str, kind: &str) -> ProbeReport {
    ProbeReport {
        container: container.into(),
        kind: kind.into(),
        success: false,
        successes: 0,
        failures: 0,
        message: "exec probes are not supported".into(),
        unsupported: true,
    }
}
pub(in crate::controllers::pod) struct Fixture {
    pub(in crate::controllers::pod) store: Store,
    pub(in crate::controllers::pod) pod: Id,
    pub(in crate::controllers::pod) machine: Id,
}
pub(in crate::controllers::pod) fn running_pod(
    policy: RestartPolicy,
    containers: Vec<Container>,
    probes: Vec<ProbeReport>,
    spec_restarts: u64,
    status_restarts: u64,
    counts: &[(&str, u32)],
) -> Fixture {
    let store = Store::in_memory().unwrap();
    let mut pod = Pod::new(
        "web",
        None,
        PodSpec {
            containers,
            restart_policy: policy,
            ..PodSpec::default()
        },
    );
    if !counts.is_empty() {
        pod.status.container_statuses = pod
            .spec
            .containers
            .iter()
            .map(|c| crate::types::ContainerStatus {
                name: c.name.clone(),
                image: c.image.clone(),
                restart_count: counts
                    .iter()
                    .find(|(n, _)| *n == c.name)
                    .map(|(_, count)| *count)
                    .unwrap_or(0),
                ..Default::default()
            })
            .collect();
    }
    let pod = store.collection::<Pod>().create(pod).unwrap();
    let mut machine = new_machine(
        "pod-web",
        None,
        MachineSpec {
            kind: MachineKind::Container,
            size: "small".into(),
            image: "img".into(),
            restarts: spec_restarts,
            ..MachineSpec::default()
        },
    );
    machine
        .meta
        .labels
        .insert(MACHINE_POD_LABEL.into(), pod.meta.id.to_string());
    machine.status.phase = Phase::Ready;
    machine.status.power = PowerState::On;
    machine.status.restarts = status_restarts;
    machine.status.host = Some(Id::new(kind::HOST));
    machine.status.addresses = vec!["10.1.0.8".into()];
    machine.status.probes = probes;
    let machine = store.collection::<Machine>().create(machine).unwrap();
    Fixture {
        store,
        pod: pod.meta.id,
        machine: machine.meta.id.clone(),
    }
}
pub(in crate::controllers::pod) async fn pass(store: &Store) {
    PodController::new(store.clone())
        .reconcile_once()
        .await
        .unwrap();
}
pub(in crate::controllers::pod) fn load(store: &Store, id: &Id) -> Pod {
    store.collection::<Pod>().get(id).unwrap()
}
pub(in crate::controllers::pod) fn reason<'a>(pod: &'a Pod, kind: &str) -> &'a str {
    pod.status
        .conditions
        .iter()
        .find(|c| c.kind == kind)
        .map(|c| c.reason.as_str())
        .unwrap()
}
