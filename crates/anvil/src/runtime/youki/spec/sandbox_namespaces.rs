// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::privilege::Privilege;
use crate::runtime::youki::privilege::RunMode;
use crate::runtime::youki::spec::network_namespace;
use std::path::Path;

pub(in crate::runtime::youki) fn sandbox_namespaces(
    mode: &RunMode,
    host_netns: Option<&Path>,
) -> Vec<serde_json::Value> {
    let mut namespaces = Vec::new();
    if matches!(&mode.privilege, Privilege::Rootless { .. }) {
        // Created here, not joined. Mappings belong on this spec only.
        namespaces.push(serde_json::json!({ "type": "user" }));
    }
    namespaces.push(serde_json::json!({ "type": "pid" }));
    namespaces.push(network_namespace(mode, host_netns));
    for kind in ["ipc", "uts", "mount"] {
        namespaces.push(serde_json::json!({ "type": kind }));
    }
    namespaces
}
