// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::PodSandbox;
use crate::types::{EnvVarSource, VolumeSource};
use std::collections::BTreeSet;
pub(in crate::controllers::pod) fn referenced_objects(
    sandbox: &PodSandbox,
) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut config_maps = BTreeSet::new();
    let mut secrets = BTreeSet::new();
    for container in &sandbox.containers {
        for env in &container.env {
            match &env.value_from {
                Some(EnvVarSource::ConfigMap { name, .. }) => {
                    config_maps.insert(name.clone());
                }
                Some(EnvVarSource::Secret { name, .. }) => {
                    secrets.insert(name.clone());
                }
                None => {}
            }
        }
    }
    for volume in &sandbox.volumes {
        match &volume.source {
            VolumeSource::ConfigMap(name) => {
                config_maps.insert(name.clone());
            }
            VolumeSource::Secret(name) => {
                secrets.insert(name.clone());
            }
            VolumeSource::EmptyDir | VolumeSource::Volume(_) => {}
        }
    }
    (config_maps, secrets)
}
