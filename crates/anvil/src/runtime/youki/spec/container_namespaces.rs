// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

pub(in crate::runtime::youki) fn container_namespaces(
    pid: u32,
    rootless: bool,
) -> Vec<serde_json::Value> {
    let join = |kind: &str, name: &str| {
        serde_json::json!({
            "type": kind,
            "path": format!("/proc/{pid}/ns/{name}"),
        })
    };
    let mut namespaces = Vec::new();
    // The user namespace is joined before the other shared namespaces.
    if rootless {
        namespaces.push(join("user", "user"));
    }
    namespaces.push(join("pid", "pid"));
    namespaces.push(join("network", "net"));
    namespaces.push(join("ipc", "ipc"));
    namespaces.push(join("uts", "uts"));
    namespaces.push(serde_json::json!({ "type": "mount" }));
    namespaces
}
