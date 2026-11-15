// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use pistack_store::Store;
use pistack_types::Id;
pub(in crate::controllers::pod) fn find_config_map(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
) -> Option<crate::types::ConfigMap> {
    let col = store.collection::<crate::types::ConfigMap>();
    if let Some(id) = Id::parse(crate::types::kinds::CONFIG_MAP, name_or_id) {
        if let Ok(cm) = col.get(&id) {
            return Some(cm);
        }
    }
    col.get_by_name(project, name_or_id).ok()
}
