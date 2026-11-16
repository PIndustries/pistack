// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::types::{ContainerStatus, PodStatus};
pub(in crate::controllers::pod) fn ensure_container_statuses(
    s: &mut PodStatus,
    names: &[(String, String)],
) {
    if s.container_statuses.len() != names.len()
        || s.container_statuses
            .iter()
            .zip(names)
            .any(|(c, (n, _))| &c.name != n)
    {
        s.container_statuses = names
            .iter()
            .map(|(n, i)| ContainerStatus {
                name: n.clone(),
                image: i.clone(),
                ..Default::default()
            })
            .collect();
    }
}
