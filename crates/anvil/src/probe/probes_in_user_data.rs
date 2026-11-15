// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::probe::sandbox_probes::SandboxProbes;
use crate::probe::spec::container_probes::ContainerProbes;

/// Probe definitions carried on a pod sandbox document. Anything that is
/// not a sandbox, or that does not parse, yields an empty list.
pub fn probes_in_user_data(raw: Option<&str>) -> Vec<ContainerProbes> {
    let Some(raw) = raw.map(str::trim).filter(|s| !s.is_empty()) else {
        return Vec::new();
    };
    let doc: SandboxProbes = match serde_json::from_str(raw) {
        Ok(doc) => doc,
        Err(_) => return Vec::new(),
    };
    if doc.kind != "PodSandbox" {
        return Vec::new();
    }
    doc.containers
        .into_iter()
        .filter(|c| c.liveness_probe.is_some() || c.readiness_probe.is_some())
        .map(|c| ContainerProbes {
            container: c.name,
            liveness: c.liveness_probe,
            readiness: c.readiness_probe,
        })
        .collect()
}
