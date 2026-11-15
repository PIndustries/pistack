// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::DEFAULT_NETWORK;
use crate::types::Pod;
use pistack_store::Store;
/// Networks a new pod machine joins. The pod's own `networks` when it names
/// any; else `default` when the project can see a network by that name;
/// else the project's only non-external network. None or several is an
/// error that says which networks the pod could name.
pub fn pick_networks(store: &Store, pod: &Pod) -> Result<Vec<String>, String> {
    if !pod.spec.networks.is_empty() {
        return Ok(pod.spec.networks.clone());
    }
    let nets = store
        .collection::<fabric::Network>()
        .list(None)
        .map_err(|e| format!("networks are not available: {e}"))?;
    let visible: Vec<&fabric::Network> = nets
        .iter()
        .filter(|n| n.meta.project.is_none() || n.meta.project == pod.meta.project)
        .collect();
    if visible.iter().any(|n| n.meta.name == DEFAULT_NETWORK) {
        return Ok(vec![DEFAULT_NETWORK.into()]);
    }
    let mut usable: Vec<String> = visible
        .iter()
        .filter(|n| !n.spec.external)
        .map(|n| n.meta.name.clone())
        .collect();
    usable.sort();
    usable.dedup();
    match usable.len() {
        1 => Ok(usable),
        0 => Err(
            "pod names no network and its project has no network that is not external; \
             create one or name one in spec.networks"
                .into(),
        ),
        _ => Err(format!(
            "pod names no network and its project has several ({}); name one in spec.networks",
            usable.join(", ")
        )),
    }
}
#[cfg(test)]
#[path = "pick_networks_tests.rs"]
mod pick_networks_tests;
