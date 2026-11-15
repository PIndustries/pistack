// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::probes_in_user_data::probes_in_user_data;

#[test]
fn sandbox_document_yields_only_containers_with_probes() {
    let raw = r#"{
        "kind": "PodSandbox",
        "containers": [
            {"name": "web", "image": "img", "readiness_probe": {"tcp_socket": {"port": 80}}},
            {"name": "side", "image": "img"},
            {"name": "job", "liveness_probe": {"exec": {"command": ["/bin/true"]}}}
        ]
    }"#;
    let found = probes_in_user_data(Some(raw));
    assert_eq!(found.len(), 2);
    assert_eq!(found[0].container, "web");
    assert!(found[0].readiness.is_some());
    assert_eq!(found[1].container, "job");
    assert!(probes_in_user_data(Some("not json")).is_empty());
    assert!(probes_in_user_data(Some(r#"{"kind":"Other"}"#)).is_empty());
}
