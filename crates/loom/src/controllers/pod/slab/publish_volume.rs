// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::{absolute_host_path, host_id, PublishedVol, SlabVolume};
use forge::Machine;
use pistack_types::Phase;
pub(in crate::controllers::pod) fn publish_volume(
    vol: &SlabVolume,
    pod_name: &str,
    machine: Option<&Machine>,
    machines: &[Machine],
) -> Result<PublishedVol, String> {
    let name = if vol.meta.name.is_empty() {
        pod_name.to_string()
    } else {
        vol.meta.name.clone()
    };
    let wait = format!("waiting for volume {name}");
    if vol.status.phase != Phase::Ready {
        return Err(wait);
    }
    let attached = vol.status.attached_to.is_some()
        || vol
            .status
            .conditions
            .iter()
            .any(|c| c.kind == "Attached" && c.status);
    if !attached {
        return Err(wait);
    }
    let path = vol
        .status
        .host_path
        .as_deref()
        .and_then(absolute_host_path)
        .or_else(|| {
            vol.status
                .backend_ref
                .as_deref()
                .and_then(absolute_host_path)
        })
        .map(str::to_string);
    let device = vol
        .status
        .device
        .as_deref()
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .map(str::to_string);
    if path.is_none() && device.is_none() {
        return Err(wait);
    }
    let attached_here =
        machine.is_some_and(|m| vol.status.attached_to.as_ref() == Some(&m.meta.id));
    let node = vol.status.host.as_deref().and_then(host_id).or_else(|| {
        let mid = vol.status.attached_to.as_ref()?;
        machines
            .iter()
            .find(|m| &m.meta.id == mid)
            .and_then(|m| m.status.host.clone())
    });
    Ok(PublishedVol {
        id: vol.meta.id.to_string(),
        name,
        path,
        device,
        node,
        attached_here,
    })
}
