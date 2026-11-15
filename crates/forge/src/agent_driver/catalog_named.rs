// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::catalog_image::CatalogImage;
use pistack_store::Store;
use pistack_types::Id;
/// The catalog image called `name_or_id` (name or id). The name is looked up
/// as `mold::Client::resolve` looks it up: the project's own image first,
/// then a cloud-wide one, then a public one of that name, newest first.
pub(super) fn catalog_named(
    store: &Store,
    project: Option<&Id>,
    name_or_id: &str,
) -> Option<CatalogImage> {
    if let Some(id) = pistack_types::Id::parse(pistack_types::kind::IMAGE, name_or_id) {
        let key = format!("img/{id}");
        let (_, bytes) = store
            .raw_scan("resources", &key)
            .ok()?
            .into_iter()
            .find(|(k, _)| k == &key)?;
        return serde_json::from_slice(&bytes).ok();
    }
    let rows: Vec<CatalogImage> = store
        .raw_scan("resources", "img/")
        .ok()?
        .into_iter()
        .filter_map(|(_, bytes)| serde_json::from_slice(&bytes).ok())
        .filter(|i: &CatalogImage| i.name == name_or_id)
        .collect();
    let own = project.map(Id::to_string);
    rows.iter()
        .find(|i| i.project == own)
        .or_else(|| rows.iter().find(|i| i.project.is_none()))
        .or_else(|| {
            rows.iter()
                .filter(|i| i.spec.visibility == "public")
                .max_by_key(|i| i.created_at.as_str())
        })
        .cloned()
}
