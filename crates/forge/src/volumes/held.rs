// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{Id, Resource};

use super::volume_spec_view::VolumeSpecView;
use super::volume_status_view::VolumeStatusView;

/// A volume that lives on one node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Held {
    /// The name the machine used for the volume.
    pub volume: String,
    pub node: String,
}

/// The volumes in `names` that are tied to a node, in the order given.
pub fn held(store: &Store, project: Option<&Id>, names: &[String]) -> Vec<Held> {
    let col = store.collection::<Resource<VolumeSpecView, VolumeStatusView>>();
    names
        .iter()
        .filter(|name| !name.trim().is_empty())
        .filter_map(|name| {
            let vol = col.resolve(project, name.trim()).ok()?;
            // An id resolves in any project. Only the machine's own counts.
            if project.is_some() && vol.meta.project.as_ref() != project {
                return None;
            }
            let node = vol.status.home_node.filter(|n| !n.is_empty())?;
            Some(Held {
                volume: name.trim().to_owned(),
                node,
            })
        })
        .collect()
}
