// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_store::Store;
use pistack_types::Id;
pub(in crate::controllers::pod) fn find_secret_id(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
) -> Option<Id> {
    if let Some(id) = Id::parse(pistack_types::kind::SECRET, name_or_id) {
        return Some(id);
    }
    store
        .collection::<keep::Secret>()
        .get_by_name(project, name_or_id)
        .ok()
        .map(|s| s.meta.id)
}
