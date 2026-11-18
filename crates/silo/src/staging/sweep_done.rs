// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use std::path::Path;
use std::time::Duration;

use super::done_dir::DONE_DIR;
use super::modified_ms::modified_ms;
use super::now_ms::now_ms;

/// Remove finished-upload marks older than `age` under one bucket
/// directory. Returns how many went.
pub(crate) async fn sweep_done(bucket_dir: &Path, age: Duration) -> usize {
    let Ok(mut entries) = tokio::fs::read_dir(bucket_dir.join(DONE_DIR)).await else {
        return 0;
    };
    let mut removed = 0;
    while let Ok(Some(entry)) = entries.next_entry().await {
        let old = modified_ms(&entry.path())
            .await
            .and_then(|ms| now_ms().checked_sub(ms))
            .is_some_and(|d| d >= age.as_millis() as u64);
        if old && tokio::fs::remove_file(entry.path()).await.is_ok() {
            removed += 1;
        }
    }
    removed
}
