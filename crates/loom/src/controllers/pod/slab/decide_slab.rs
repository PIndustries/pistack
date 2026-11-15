// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::slab::{publish_volume, slab_refs, SlabBind, SlabHold, SlabVolume};
use crate::types::Pod;
use forge::Machine;
use std::collections::BTreeMap;
pub(in crate::controllers::pod) fn decide_slab(
    pod: &Pod,
    machine: Option<&Machine>,
    machines: &[Machine],
    vols: &[SlabVolume],
) -> SlabHold {
    let named = slab_refs(pod);
    if named.is_empty() {
        return SlabHold::None;
    }
    let mut published = Vec::new();
    for (pod_name, id) in named {
        let Some(vol) = vols.iter().find(|v| &v.meta.id == id) else {
            return SlabHold::Wait(format!("waiting for volume {pod_name}"));
        };
        match publish_volume(vol, pod_name, machine, machines) {
            Ok(item) => published.push(item),
            Err(message) => return SlabHold::Wait(message),
        }
    }
    let placed = machine.and_then(|m| m.status.host.clone());
    let mut node = placed.clone();
    for item in &published {
        if let Some(host) = &placed {
            let here = item.attached_here || item.node.as_ref() == Some(host);
            if !here {
                return SlabHold::Wait(format!(
                    "volume {} is not attached on this node",
                    item.name
                ));
            }
            node = Some(host.clone());
        } else if let Some(found) = &item.node {
            match &node {
                None => node = Some(found.clone()),
                Some(have) if have == found => {}
                Some(_) => {
                    return SlabHold::Wait(format!(
                        "volume {} is not attached on this node",
                        item.name
                    ));
                }
            }
        } else {
            return SlabHold::Wait(format!("waiting for volume {}", item.name));
        }
    }
    let Some(node) = node else {
        return SlabHold::Wait(format!("waiting for volume {}", published[0].name));
    };
    let mut paths = BTreeMap::new();
    let mut devices = BTreeMap::new();
    let mut ids = Vec::new();
    let mut names = BTreeMap::new();
    for item in &published {
        ids.push(item.id.clone());
        names.insert(item.id.clone(), item.name.clone());
        if let Some(path) = &item.path {
            paths.insert(item.id.clone(), path.clone());
        } else if let Some(dev) = &item.device {
            devices.insert(item.id.clone(), dev.clone());
        }
    }
    ids.sort();
    SlabHold::Ready(SlabBind {
        paths,
        devices,
        node,
        mounted: ids.join(","),
        names,
    })
}
