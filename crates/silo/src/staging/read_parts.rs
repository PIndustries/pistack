// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;

use super::modified_ms::modified_ms;
use super::staged_part::StagedPart;

/// The parts in a staging directory that have their MD5 beside them.
pub(crate) async fn read_parts(dir: &Path) -> Vec<StagedPart> {
    let mut parts = Vec::new();
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return parts;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(number) = name
            .strip_prefix("part-")
            .and_then(|n| n.parse::<u32>().ok())
        else {
            continue;
        };
        let Ok(md5) = tokio::fs::read_to_string(dir.join(format!("{name}.md5"))).await else {
            continue;
        };
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        parts.push(StagedPart {
            number,
            size: meta.len(),
            md5: md5.trim().to_owned(),
            modified_ms: modified_ms(&entry.path()).await.unwrap_or(0),
        });
    }
    parts.sort_by_key(|p| p.number);
    parts
}
