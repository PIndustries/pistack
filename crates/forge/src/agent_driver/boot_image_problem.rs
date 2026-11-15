// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::catalog_named::catalog_named;
use pistack_store::Store;
use pistack_types::Id;
use std::path::Path;
/// Why `image` cannot be a machine's boot image, or `None` when it can: a
/// path the host resolves itself, or a catalog image that is ready, visible
/// to `project` and carries a file or a digest. Unlike a start, which sends
/// an unknown name to the host and fails there, a rebuild answers now.
pub(crate) fn boot_image_problem(
    store: &Store,
    project: Option<&Id>,
    image: &str,
) -> Option<String> {
    if image.trim().is_empty() {
        return Some("the machine names no image".to_owned());
    }
    if Path::new(image).is_absolute() {
        return None;
    }
    match catalog_named(store, project, image) {
        None => Some(format!("image '{image}' not found")),
        Some(img) if img.status.phase != "ready" => Some(format!(
            "image '{image}' is {phased}, not ready",
            phased = img.status.phase
        )),
        Some(img) if !img.usable_by(store, project) => {
            Some(format!("image '{image}' is not visible to this project"))
        }
        Some(img) if img.local_file().is_none() && img.source().is_none() => Some(format!(
            "image '{image}' has no file and no digest the host can use"
        )),
        Some(_) => None,
    }
}
