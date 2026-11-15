// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::{PodSandbox, ProjectedFiles};
use crate::controllers::pod::projections::{
    find_config_map, find_secret_id, project_secret, referenced_objects, remember,
};
use pistack_store::Store;
/// Copy ConfigMap and Secret bodies the pod references into the sandbox
/// document the agent receives. A missing object is left out; the runtime
/// then starts without that value.
pub(in crate::controllers::pod) fn fill_projections(store: &Store, sandbox: &mut PodSandbox) {
    let project = sandbox.pod.project.clone();
    let (config_maps, secrets) = referenced_objects(sandbox);
    for name in config_maps {
        let Some(cm) = find_config_map(store, project.as_ref(), &name) else {
            tracing::warn!(config_map = %name, "loom: config map is not available for the sandbox");
            continue;
        };
        remember(
            &mut sandbox.config_maps,
            &cm.meta.name,
            &cm.meta.id.to_string(),
            ProjectedFiles {
                data: cm.spec.data.clone(),
                binary_data: cm.spec.binary_data.clone(),
            },
        );
    }
    for name in secrets {
        let Some(id) = find_secret_id(store, project.as_ref(), &name) else {
            tracing::warn!(secret = %name, "loom: secret is not available for the sandbox");
            continue;
        };
        let bytes = match keep::read_payload(store, &id) {
            Ok(bytes) => bytes,
            Err(e) => {
                tracing::warn!(secret = %name, error = %e, "loom: secret payload is not available for the sandbox");
                continue;
            }
        };
        let label = store
            .collection::<keep::Secret>()
            .get(&id)
            .map(|s| s.meta.name)
            .unwrap_or_default();
        remember(
            &mut sandbox.secrets,
            &label,
            &id.to_string(),
            project_secret(&bytes),
        );
    }
}
#[cfg(test)]
#[path = "fill_projections_tests.rs"]
mod fill_projections_tests;
