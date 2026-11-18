// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use md5::{Digest, Md5};
use std::path::Path;
use std::time::{Duration, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

use super::staged_part::StagedPart;
use super::stall::STALL;

/// Stream a part to a temp file beside `dest` and rename it only when its
/// length and MD5 are the ones the list gave. The file keeps the time the
/// part was written on the server it came from.
pub(crate) async fn download(
    mut resp: reqwest::Response,
    part: &StagedPart,
    dest: &Path,
) -> Result<(), String> {
    let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    let mut guard = crate::client::TmpGuard::new(tmp.clone());
    let mut file = tokio::fs::File::create(&tmp)
        .await
        .map_err(|e| e.to_string())?;
    let mut hasher = Md5::new();
    let mut got: u64 = 0;
    loop {
        let chunk = tokio::time::timeout(STALL, resp.chunk())
            .await
            .map_err(|_| "transfer stalled".to_owned())?
            .map_err(|e| e.to_string())?;
        let Some(chunk) = chunk else { break };
        got += chunk.len() as u64;
        if got > part.size {
            return Err(format!("more than the {} bytes listed", part.size));
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
    }
    if got != part.size {
        return Err(format!("{got} bytes, the list gives {}", part.size));
    }
    let md5: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if md5 != part.md5 {
        return Err("MD5 does not match the list".to_owned());
    }
    file.flush().await.map_err(|e| e.to_string())?;
    file.sync_all().await.map_err(|e| e.to_string())?;
    if part.modified_ms > 0 {
        let when = UNIX_EPOCH + Duration::from_millis(part.modified_ms);
        let std_file = file.into_std().await;
        let _ = tokio::task::spawn_blocking(move || std_file.set_modified(when)).await;
    } else {
        drop(file);
    }
    tokio::fs::rename(&tmp, dest)
        .await
        .map_err(|e| e.to_string())?;
    guard.disarm();
    Ok(())
}
