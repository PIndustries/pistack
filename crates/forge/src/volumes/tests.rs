// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_store::Store;
use pistack_types::{kind, Id, Resource};

use crate::types::Host;

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
