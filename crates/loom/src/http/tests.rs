// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;

use super::*;
use crate::http::parse_query::parse_query;
use crate::http::validate_pod_spec::validate_pod_spec;
use crate::http::validate_selector::validate_selector;

#[test]
fn service_node_port_validation() {
    let spec = |kind: ServiceKind, ports: &[(u16, Option<u16>)]| ServiceSpec {
        ports: ports
            .iter()
            .map(|(port, node_port)| ServicePort {
                port: *port,
                node_port: *node_port,
                ..ServicePort::default()
            })
            .collect(),
        kind,
        ..ServiceSpec::default()
    };
    assert!(spec(ServiceKind::NodePort, &[(80, Some(30080))])
        .validate(None)
        .is_ok());
    assert!(spec(ServiceKind::LoadBalancer, &[(80, None)])
        .validate(None)
        .is_ok());
    assert!(spec(ServiceKind::ClusterIp, &[(80, Some(30080))])
        .validate(None)
        .is_err());
    assert!(spec(ServiceKind::NodePort, &[(80, Some(8080))])
        .validate(None)
        .is_err());
    let mut shared = spec(
        ServiceKind::NodePort,
        &[(53, Some(30053)), (53, Some(30053))],
    );
    shared.ports[1].protocol = Protocol::Udp;
    assert!(shared.validate(None).is_ok());
    assert!(spec(
        ServiceKind::NodePort,
        &[(80, Some(30080)), (81, Some(30080))]
    )
    .validate(None)
    .is_err());
}

#[test]
fn query_parses() {
    let q = parse_query("limit=5&labels=app%3Dweb&name=a+b").unwrap();
    assert_eq!(q.limit, Some(5));
    assert_eq!(q.labels.as_deref(), Some("app=web"));
    assert_eq!(q.name.as_deref(), Some("a b"));
    assert!(parse_query("limit=x").is_err());
}

#[test]
fn selector_defaults_and_must_match() {
    let mut t = PodTemplate::default();
    t.labels.insert("app".into(), "web".into());
    let mut sel = Selector::default();
    validate_selector(&mut sel, &t).unwrap();
    assert_eq!(sel.match_labels.get("app").unwrap(), "web");
    let mut bad = Selector::from_labels(&[("app".to_string(), "db".to_string())].into());
    assert!(validate_selector(&mut bad, &t).is_err());
    assert!(validate_selector(&mut Selector::default(), &PodTemplate::default()).is_err());
}

#[test]
fn pod_spec_validation() {
    assert!(validate_pod_spec(&PodSpec::default()).is_err());
    let mut spec = PodSpec {
        containers: vec![Container {
            name: "web".into(),
            image: "nginx".into(),
            ..Default::default()
        }],
        ..Default::default()
    };
    assert!(validate_pod_spec(&spec).is_ok());
    spec.containers[0].volume_mounts.push(VolumeMount {
        name: "data".into(),
        mount_path: "/data".into(),
        ..Default::default()
    });
    assert!(validate_pod_spec(&spec).is_err());
    spec.volumes.push(PodVolume {
        name: "data".into(),
        source: VolumeSource::EmptyDir,
    });
    assert!(validate_pod_spec(&spec).is_ok());
}

#[test]
fn probe_validation_rejects_a_bad_handler() {
    let mut spec = PodSpec {
        containers: vec![Container {
            name: "web".into(),
            image: "nginx".into(),
            liveness_probe: Some(Probe {
                http_get: Some(HttpGetProbe {
                    path: "/".into(),
                    port: 0,
                    host: None,
                    scheme: None,
                }),
                ..Probe::default()
            }),
            ..Container::default()
        }],
        ..PodSpec::default()
    };
    assert!(validate_pod_spec(&spec).is_err());

    spec.containers[0].liveness_probe = Some(Probe {
        http_get: Some(HttpGetProbe {
            path: "/".into(),
            port: 8080,
            host: None,
            scheme: None,
        }),
        tcp_socket: Some(TcpSocketProbe { port: 8080 }),
        ..Probe::default()
    });
    assert!(validate_pod_spec(&spec).is_err());

    spec.containers[0].liveness_probe = Some(Probe {
        exec: Some(ExecProbe { command: vec![] }),
        ..Probe::default()
    });
    assert!(validate_pod_spec(&spec).is_err());

    spec.containers[0].liveness_probe = Some(Probe {
        exec: Some(ExecProbe {
            command: vec!["".into()],
        }),
        ..Probe::default()
    });
    let empty = validate_pod_spec(&spec).unwrap_err();
    assert!(
        empty.message.contains("exec command is empty"),
        "{}",
        empty.message
    );
    assert!(empty.message.contains("web"), "{}", empty.message);
    assert!(
        empty.message.contains("liveness_probe"),
        "{}",
        empty.message
    );

    spec.containers[0].liveness_probe = Some(Probe {
        exec: Some(ExecProbe {
            command: vec!["/bin/echo".into(), format!("a{}b", '\0')],
        }),
        ..Probe::default()
    });
    let nul = validate_pod_spec(&spec).unwrap_err();
    assert!(nul.message.contains("NUL byte"), "{}", nul.message);
    assert!(nul.message.contains("web"), "{}", nul.message);

    spec.containers[0].liveness_probe = Some(Probe {
        exec: Some(ExecProbe {
            command: vec!["/bin/true".into()],
        }),
        ..Probe::default()
    });
    assert!(validate_pod_spec(&spec).is_ok());

    spec.containers[0].readiness_probe = Some(Probe {
        http_get: Some(HttpGetProbe {
            path: "/ready".into(),
            port: 8080,
            host: Some("bad host".into()),
            scheme: None,
        }),
        ..Probe::default()
    });
    spec.containers[0].liveness_probe = None;
    assert!(validate_pod_spec(&spec).is_err());

    spec.containers[0].readiness_probe = Some(Probe {
        tcp_socket: Some(TcpSocketProbe { port: 80 }),
        ..Probe::default()
    });
    assert!(validate_pod_spec(&spec).is_ok());
}
