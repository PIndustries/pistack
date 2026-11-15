// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_types::Id;
use std::collections::BTreeMap;
/// Paths and the node to pin, once every slab volume on the pod is attached.
pub(in crate::controllers::pod) struct SlabBind {
    pub(in crate::controllers::pod) paths: BTreeMap<String, String>,
    pub(in crate::controllers::pod) devices: BTreeMap<String, String>,
    pub(in crate::controllers::pod) node: Id,
    /// Sorted volume ids, comma-separated.
    pub(in crate::controllers::pod) mounted: String,
    pub(in crate::controllers::pod) names: BTreeMap<String, String>,
}
