// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::catalog_image::CatalogImage;
use pistack_store::Store;
use pistack_types::Id;
/// The ready catalog image called `name` that a machine in `project` may
/// boot. The project's own image wins over one shared with it or public.
/// Among those, one whose file is on this server wins over one that only
/// has a digest.
pub(super) fn catalog_image(
    store: &Store,
    project: Option<&Id>,
    name: &str,
) -> Option<CatalogImage> {
    let rows = store.raw_scan("resources", "img/").ok()?;
    let own = project.map(Id::to_string);
    let mut best: Option<(u8, CatalogImage)> = None;
    for (_, bytes) in rows {
        let Ok(view) = serde_json::from_slice::<CatalogImage>(&bytes) else {
            continue;
        };
        if view.name != name || view.status.phase != "ready" {
            continue;
        }
        let has_file = view.local_file().is_some();
        if !has_file && view.source().is_none() {
            continue;
        }
        if !view.usable_by(store, project) {
            continue;
        }
        let foreign = own.is_some() && view.project != own;
        let rank = u8::from(foreign) * 2 + u8::from(!has_file);
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, view));
        }
    }
    best.map(|(_, view)| view)
}
