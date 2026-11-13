// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Read just enough of a slab volume to know which host can attach it.
//! forge does not depend on slab; this view ignores every field it does
//! not need.
//!
//! slab records `status.home_node` on a local file or an LVM volume: the
//! node whose disk holds the bytes. Such a volume attaches there and
//! nowhere else, so a machine that names one in `spec.near_volumes` is
//! placed on that node or not at all. An RBD volume has no home node and does
//! not limit placement. A name that matches no volume is left to the
//! attach to report.

use pistack_store::Store;
use pistack_types::{kind, ApiError, ApiResult, Id, Resource, Spec};
use serde::{Deserialize, Serialize};

use crate::types::Host;

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct VolumeSpecView {}

impl Spec for VolumeSpecView {
    const KIND: &'static str = kind::VOLUME;
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
struct VolumeStatusView {
    #[serde(default)]
    home_node: Option<String>,
}

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

/// The hosts that can attach every volume in `held`.
///
/// `None` when no volume is tied to a node, so the caller keeps its own
/// list. An error when the volumes are on different nodes, or when the
/// node that holds them is not among `hosts`.
pub fn hosts_near(held: &[Held], hosts: &[Host]) -> ApiResult<Option<Vec<Host>>> {
    let Some(first) = held.first() else {
        return Ok(None);
    };
    if let Some(other) = held.iter().find(|h| h.node != first.node) {
        return Err(ApiError::unavailable(format!(
            "no host can attach both volume {} (held on node {}) and volume {} (held on node {})",
            first.volume, first.node, other.volume, other.node
        )));
    }
    let near: Vec<Host> = hosts
        .iter()
        .filter(|h| h.spec.node_name == first.node)
        .cloned()
        .collect();
    if near.is_empty() {
        return Err(ApiError::unavailable(format!(
            "no host can attach volume {}: it is held on node {}, which is not one of the {} hosts this machine can use",
            first.volume,
            first.node,
            hosts.len()
        ))
        .with_detail("home_node", first.node.clone())
        .with_detail("hosts", hosts.len() as u64));
    }
    Ok(Some(near))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pistack_types::error::ErrorCode;

    fn host(name: &str) -> Host {
        crate::types::new_host(
            name,
            name,
            Default::default(),
            Vec::new(),
            Default::default(),
        )
    }

    fn volume(store: &Store, name: &str, project: &Id, home: Option<&str>) {
        let mut vol = Resource::<VolumeSpecView, VolumeStatusView>::new(
            name,
            Some(project.clone()),
            VolumeSpecView {},
        );
        vol.status.home_node = home.map(str::to_owned);
        store
            .collection::<Resource<VolumeSpecView, VolumeStatusView>>()
            .create(vol)
            .unwrap();
    }

    #[test]
    fn only_volumes_with_a_home_node_limit_placement() {
        let store = Store::in_memory().unwrap();
        let project = Id::new(kind::PROJECT);
        volume(&store, "local", &project, Some("alpha"));
        volume(&store, "ceph", &project, None);

        let names = vec!["ceph".to_owned(), "local".to_owned(), "missing".to_owned()];
        let found = held(&store, Some(&project), &names);
        assert_eq!(
            found,
            vec![Held {
                volume: "local".into(),
                node: "alpha".into()
            }]
        );
        // Another project's volume of the same name is not this machine's.
        assert!(held(&store, Some(&Id::new(kind::PROJECT)), &names).is_empty());
        let by_id: Vec<String> = store
            .collection::<Resource<VolumeSpecView, VolumeStatusView>>()
            .list(None)
            .unwrap()
            .iter()
            .map(|v| v.meta.id.to_string())
            .collect();
        assert!(held(&store, Some(&Id::new(kind::PROJECT)), &by_id).is_empty());
        assert_eq!(held(&store, Some(&project), &by_id).len(), 1);

        let hosts = vec![host("alpha"), host("beta")];
        assert!(hosts_near(&[], &hosts).unwrap().is_none());
        let near = hosts_near(&found, &hosts).unwrap().unwrap();
        assert_eq!(near.len(), 1);
        assert_eq!(near[0].spec.node_name, "alpha");
    }

    #[test]
    fn a_volume_on_an_unusable_node_is_an_error_that_names_it() {
        let found = vec![Held {
            volume: "local".into(),
            node: "alpha".into(),
        }];
        let err = hosts_near(&found, &[host("beta"), host("gamma")]).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("volume local"), "{}", err.message);
        assert!(err.message.contains("node alpha"), "{}", err.message);
        assert!(err.message.contains("2 hosts"), "{}", err.message);
    }

    #[test]
    fn volumes_on_two_nodes_cannot_be_placed() {
        let found = vec![
            Held {
                volume: "a".into(),
                node: "alpha".into(),
            },
            Held {
                volume: "b".into(),
                node: "beta".into(),
            },
        ];
        let err = hosts_near(&found, &[host("alpha"), host("beta")]).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unavailable);
        assert!(err.message.contains("volume a"), "{}", err.message);
        assert!(err.message.contains("node beta"), "{}", err.message);
    }
}
