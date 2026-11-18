// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

use super::marker::Marker;

pub(crate) async fn read_marker_file(path: &Path) -> Option<Marker> {
    let text = tokio::fs::read_to_string(path).await.ok()?;
    let (sha256, key) = text.split_once('\n')?;
    Some(Marker {
        sha256: sha256.to_owned(),
        key: key.to_owned(),
    })
}
