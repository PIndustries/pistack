// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::runtime::youki::image::rel_under;
use crate::runtime::youki::volumes::PreparedVolume;

pub(in crate::runtime::youki) fn mount_source(
    prepared: &PreparedVolume,
    sub_path: &str,
) -> Option<(String, bool)> {
    let (source, directory) = if sub_path.is_empty() {
        (prepared.host_path.clone(), prepared.directory)
    } else {
        let rel = rel_under(sub_path)?;
        let source = prepared.host_path.join(rel);
        let directory = std::fs::metadata(&source)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        (source, directory)
    };
    Some((source.to_string_lossy().into_owned(), directory))
}
