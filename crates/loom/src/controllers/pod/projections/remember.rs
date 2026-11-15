// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors
use crate::controllers::pod::document::ProjectedFiles;
use std::collections::BTreeMap;
pub(in crate::controllers::pod) fn remember(
    map: &mut BTreeMap<String, ProjectedFiles>,
    name: &str,
    id: &str,
    files: ProjectedFiles,
) {
    if !name.is_empty() {
        map.insert(name.to_owned(), files.clone());
    }
    if !id.is_empty() && id != name {
        map.insert(id.to_owned(), files);
    }
}
