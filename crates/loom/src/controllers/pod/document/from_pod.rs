// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::{
    PodIdentity, PodSandbox, SANDBOX_API_VERSION, SANDBOX_KIND,
};
use crate::controllers::pod::networks::networks_of;
use crate::types::Pod;
use std::collections::BTreeMap;
impl PodSandbox {
    pub fn from_pod(pod: &Pod) -> Self {
        Self {
            api_version: SANDBOX_API_VERSION.into(),
            kind: SANDBOX_KIND.into(),
            pod: PodIdentity {
                id: pod.meta.id.clone(),
                name: pod.meta.name.clone(),
                project: pod.meta.project.clone(),
                labels: pod.meta.labels.clone(),
                annotations: pod.meta.annotations.clone(),
            },
            restart_policy: pod.spec.restart_policy,
            containers: pod.spec.containers.clone(),
            volumes: pod.spec.volumes.clone(),
            service_account: pod.spec.service_account.clone(),
            networks: networks_of(pod),
            config_maps: BTreeMap::new(),
            secrets: BTreeMap::new(),
            volume_paths: BTreeMap::new(),
            volume_devices: BTreeMap::new(),
        }
    }
}
