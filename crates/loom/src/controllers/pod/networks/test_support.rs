// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::{Pod, PodSpec};
use pistack_store::Store;
use pistack_types::{kind, Id, Meta};
use std::sync::Arc;

pub(in crate::controllers::pod) fn net(
    store: &Store,
    name: &str,
    project: Option<Id>,
    external: bool,
) {
    fabric::Client::new(store.clone(), Arc::new(fabric::MockBackend::new()))
        .create_network(
            Meta::new(kind::NETWORK, name, project),
            fabric::NetworkSpec {
                external,
                ..fabric::NetworkSpec::default()
            },
        )
        .unwrap();
}
pub(in crate::controllers::pod) fn pod(project: &Id, networks: &[&str]) -> Pod {
    Pod::new(
        "web",
        Some(project.clone()),
        PodSpec {
            networks: networks.iter().map(|n| n.to_string()).collect(),
            ..PodSpec::default()
        },
    )
}
