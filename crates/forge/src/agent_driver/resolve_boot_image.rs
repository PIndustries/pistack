// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use super::catalog_image_use::catalog_image;
use pistack_rpc::proto::ImageSource;
use pistack_store::Store;
use pistack_types::Id;
use std::path::Path;
/// The image string for the wire, and where an agent can fetch it.
///
/// An absolute path is sent as it is. A bare name that matches a ready
/// catalog image the machine's project may use becomes that image's file
/// on this server when there is one, and carries the image's id and digest
/// so a host that does not have the file downloads it. Anything else is
/// sent unchanged, so an image the project cannot see fails on the host
/// the same way a name that is not in the catalog does.
pub(super) fn resolve_boot_image(
    store: Option<&Store>,
    project: Option<&Id>,
    image: &str,
) -> (String, Option<ImageSource>) {
    if Path::new(image).is_absolute() {
        return (image.to_owned(), None);
    }
    let Some(found) = store.and_then(|store| catalog_image(store, project, image)) else {
        return (image.to_owned(), None);
    };
    let source = found.source();
    (
        found.local_file().unwrap_or_else(|| image.to_owned()),
        source,
    )
}
