// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use chrono::{DateTime, Utc};
use std::path::Path;

pub(crate) async fn mtime(path: &Path) -> Option<DateTime<Utc>> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    meta.modified().ok().map(DateTime::<Utc>::from)
}
