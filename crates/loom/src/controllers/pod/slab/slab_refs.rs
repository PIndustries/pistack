// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::{Pod, VolumeSource};
use pistack_types::Id;
pub(in crate::controllers::pod) fn slab_refs(pod: &Pod) -> Vec<(&str, &Id)> {
    pod.spec
        .volumes
        .iter()
        .filter_map(|v| match &v.source {
            VolumeSource::Volume(id) => Some((v.name.as_str(), id)),
            _ => None,
        })
        .collect()
}
