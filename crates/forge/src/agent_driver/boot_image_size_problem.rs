// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::catalog_named::catalog_named;
use pistack_store::Store;
use pistack_types::Id;
use std::path::Path;
/// Why `image` cannot become the root disk of a machine of `disk_gib`, or
/// `None` when it fits or the store knows nothing to size it by. A disk of
/// 0 takes the default of 10 GiB, as the host's provision does.
pub(crate) fn boot_image_size_problem(
    store: &Store,
    project: Option<&Id>,
    image: &str,
    disk_gib: u64,
) -> Option<String> {
    if Path::new(image).is_absolute() {
        return None;
    }
    let img = catalog_named(store, project, image)?;
    let want = img.min_disk_gib();
    let disk = if disk_gib == 0 { 10 } else { disk_gib };
    (want > disk)
        .then(|| format!("image '{image}' needs a {want} GiB disk; the size gives {disk} GiB"))
}
