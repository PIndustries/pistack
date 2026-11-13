// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Multipart staging on more than one control-plane server.
//!
//! An unfinished multipart upload is files on disk, not store records
//! (see [`crate::multipart`]), so nothing replicates it. Without this
//! module an upload in flight when the leader changes is gone: the new
//! leader answers `NoSuchUpload` and the client starts again.
//!
//! * **Copy before the answer.** CreateMultipartUpload and UploadPart are
//!   answered only after one other server holds the upload's `meta.json`
//!   or the part, the same rule and the same `put_copies` setting as a
//!   put. The server that took the request calls
//!   `POST /silo/peer/uploads/{bucket-id}/{upload-id}[/{part}]` on each
//!   peer in turn; the peer fetches from the others and checks the part's
//!   length and MD5 before it answers.
//! * **Background pull.** [`sync`] runs beside the object pull on every
//!   server. It reads `GET /silo/peer/uploads` from each peer and takes
//!   the uploads and parts it lacks, so a third server catches up and a
//!   server that was down gets what it missed.
//! * **Finished uploads.** Complete and abort leave a small file,
//!   `{bucket}/.uploads-done/{upload-id}`. A server that has it never
//!   takes that upload again, and one that sees it in a peer's list
//!   removes its own copy. Without it a finished upload would be copied
//!   back from a server that had not heard yet. [`crate::gc`] removes the
//!   file once it is twice as old as an upload may get.
//!
//! A part can be uploaded again under the same number. When two servers
//! hold different bytes for a part, the one written later wins: the list
//! carries each part's modification time and a fetched part keeps it.

use crate::client::Client;
use crate::multipart::{io_err, scan_staged, UploadMeta};
use crate::replica::Target;
use crate::types::Bucket;
use md5::{Digest, Md5};
use pistack_types::{ApiError, ApiResult, Id, Spec};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;

/// What one server stages, as `GET /silo/peer/uploads` returns it.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Staged {
    #[serde(default)]
    pub uploads: Vec<StagedUpload>,
    /// Uploads this server knows were completed or aborted.
    #[serde(default)]
    pub done: Vec<DoneUpload>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagedUpload {
    pub bucket: String,
    pub upload_id: String,
    pub meta: UploadMeta,
    #[serde(default)]
    pub parts: Vec<StagedPart>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StagedPart {
    pub number: u32,
    pub size: u64,
    /// Hex MD5 of the part, its ETag.
    pub md5: String,
    /// When the part was written, in milliseconds since the Unix epoch.
    #[serde(default)]
    pub modified_ms: u64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DoneUpload {
    pub bucket: String,
    pub upload_id: String,
}

/// Which files of an upload to take from a peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Want {
    /// The staging directory and `meta.json` only.
    Meta,
    /// That, and one part.
    Part(u32),
    /// That, and every part.
    All,
}

/// What one background pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StagedReport {
    /// Parts fetched from a peer.
    pub fetched: usize,
    /// Uploads removed because a peer saw them finish.
    pub removed: usize,
}

impl Client {
    fn done_dir(&self, bucket: &Id) -> PathBuf {
        self.root().join(bucket.to_string()).join(DONE_DIR)
    }

    fn done_path(&self, bucket: &Id, upload_id: &str) -> ApiResult<PathBuf> {
        crate::multipart::validate_upload_id(upload_id)?;
        Ok(self.done_dir(bucket).join(upload_id))
    }

    /// Record that an upload was completed or aborted.
    pub(crate) async fn mark_upload_done(&self, bucket: &Id, upload_id: &str) -> ApiResult<()> {
        let path = self.done_path(bucket, upload_id)?;
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
        }
        tokio::fs::write(&path, b"").await.map_err(io_err)
    }

    async fn upload_is_done(&self, bucket: &Id, upload_id: &str) -> bool {
        match self.done_path(bucket, upload_id) {
            Ok(path) => tokio::fs::try_exists(&path).await.unwrap_or(false),
            Err(_) => false,
        }
    }

    /// Everything this server stages, or one upload when `only` names it.
    pub async fn staged(&self, only: Option<(&Id, &str)>) -> ApiResult<Staged> {
        let mut out = Staged::default();
        let buckets = self.store().collection::<Bucket>().list(None)?;
        for bucket in &buckets {
            let id = &bucket.meta.id;
            if only.is_some_and(|(b, _)| b != id) {
                continue;
            }
            for upload in scan_staged(&self.uploads_root(id)).await? {
                if only.is_some_and(|(_, u)| u != upload.upload_id) {
                    continue;
                }
                let Some(meta) = upload.meta else {
                    continue;
                };
                out.uploads.push(StagedUpload {
                    bucket: id.to_string(),
                    upload_id: upload.upload_id,
                    meta,
                    parts: read_parts(&upload.dir).await,
                });
            }
            let Ok(mut done) = tokio::fs::read_dir(self.done_dir(id)).await else {
                continue;
            };
            while let Ok(Some(entry)) = done.next_entry().await {
                let upload_id = entry.file_name().to_string_lossy().into_owned();
                if only.is_some_and(|(_, u)| u != upload_id) {
                    continue;
                }
                out.done.push(DoneUpload {
                    bucket: id.to_string(),
                    upload_id,
                });
            }
        }
        Ok(out)
    }

    /// Open a staged part this server holds, and its length.
    pub async fn open_staged_part(
        &self,
        bucket: &Id,
        upload_id: &str,
        number: u32,
    ) -> ApiResult<(tokio::fs::File, u64)> {
        let missing = || ApiError::not_found("no such part on this server");
        let path = self
            .upload_dir(bucket, upload_id)
            .map_err(|_| missing())?
            .join(format!("part-{number}"));
        let file = tokio::fs::File::open(&path).await.map_err(|_| missing())?;
        let size = file.metadata().await.map_err(io_err)?.len();
        Ok((file, size))
    }

    /// Take from the peer `from` the files of `upload` this server
    /// lacks. Returns how many parts were fetched.
    async fn take(
        &self,
        from: &Target,
        upload: &StagedUpload,
        want: Want,
    ) -> Result<usize, String> {
        let peers = self.peers().ok_or("no peers")?;
        let bucket =
            Id::parse(crate::types::BucketSpec::KIND, &upload.bucket).ok_or("not a bucket id")?;
        if self.store().collection::<Bucket>().get(&bucket).is_err() {
            return Err("no such bucket here".into());
        }
        if self.upload_is_done(&bucket, &upload.upload_id).await {
            return Ok(0);
        }
        let dir = self
            .upload_dir(&bucket, &upload.upload_id)
            .map_err(|e| e.message)?;
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        let meta_path = dir.join("meta.json");
        if !tokio::fs::try_exists(&meta_path).await.unwrap_or(false) {
            let bytes = serde_json::to_vec_pretty(&upload.meta).map_err(|e| e.to_string())?;
            write_atomic(&meta_path, &bytes)
                .await
                .map_err(|e| e.to_string())?;
        }

        let here = read_parts(&dir).await;
        let mut fetched = 0;
        for part in &upload.parts {
            match want {
                Want::Meta => break,
                Want::Part(n) if n != part.number => continue,
                _ => {}
            }
            let mine = here.iter().find(|p| p.number == part.number);
            let stale = match mine {
                None => true,
                Some(p) => p.md5 != part.md5 && part.modified_ms > p.modified_ms,
            };
            if !stale {
                continue;
            }
            let path = format!(
                "/silo/peer/uploads/{}/{}/{}",
                upload.bucket, upload.upload_id, part.number
            );
            let dest = dir.join(format!("part-{}", part.number));
            let resp = peers.get(from, &path).await?;
            download(resp, part, &dest).await?;
            tokio::fs::write(dir.join(format!("part-{}.md5", part.number)), &part.md5)
                .await
                .map_err(|e| e.to_string())?;
            fetched += 1;
        }
        Ok(fetched)
    }

    /// Fetch one upload from whichever peer has it. The handler behind
    /// `POST /silo/peer/uploads/...`: `unavailable` unless this server
    /// ends up holding what was asked for.
    pub async fn pull_staged(&self, bucket: &Id, upload_id: &str, want: Want) -> ApiResult<()> {
        let peers = self
            .peers()
            .ok_or_else(|| ApiError::unavailable("this server has no peers to fetch from"))?;
        let mut last = String::from("no other server");
        for from in peers.targets() {
            let base = &from.0;
            let path = format!("/silo/peer/uploads/{bucket}/{upload_id}");
            let found = match peers.get(&from, &path).await {
                Ok(resp) => resp.json::<Staged>().await.map_err(|e| e.to_string()),
                Err(e) => Err(e),
            };
            let upload = match found {
                Ok(list) => list.uploads.into_iter().next(),
                Err(e) => {
                    last = format!("{base}: {e}");
                    continue;
                }
            };
            let Some(upload) = upload else {
                last = format!("{base}: does not stage that upload");
                continue;
            };
            let wanted = match want {
                Want::Part(n) => upload.parts.iter().find(|p| p.number == n).cloned(),
                _ => None,
            };
            if let Err(e) = self.take(&from, &upload, want).await {
                last = format!("{base}: {e}");
                continue;
            }
            if self.holds(bucket, upload_id, want, wanted.as_ref()).await {
                return Ok(());
            }
            last = format!("{base}: did not have it");
        }
        Err(ApiError::unavailable(format!(
            "could not copy the upload here (last: {last})"
        )))
    }

    async fn holds(
        &self,
        bucket: &Id,
        upload_id: &str,
        want: Want,
        part: Option<&StagedPart>,
    ) -> bool {
        let Ok(dir) = self.upload_dir(bucket, upload_id) else {
            return false;
        };
        if !tokio::fs::try_exists(dir.join("meta.json"))
            .await
            .unwrap_or(false)
        {
            return false;
        }
        match (want, part) {
            (Want::Part(n), Some(part)) => read_parts(&dir)
                .await
                .iter()
                .any(|p| p.number == n && p.md5 == part.md5),
            (Want::Part(_), None) => false,
            _ => true,
        }
    }

    /// Have other servers copy an upload's `meta.json` (`part` is `None`)
    /// or one of its parts before the request that staged it is answered.
    ///
    /// Follows [`Client::with_put_copies`]. Ok when there is nobody to
    /// ask; `unavailable` when there were peers and too few took it.
    pub(crate) async fn push_staged(
        &self,
        bucket: &Id,
        upload_id: &str,
        part: Option<u32>,
        size: u64,
    ) -> ApiResult<()> {
        let Some(peers) = self.peers() else {
            return Ok(());
        };
        let urls = peers.targets();
        let wanted = (self.put_copies().saturating_sub(1) as usize).min(urls.len());
        if wanted == 0 {
            return Ok(());
        }
        let wait = crate::replica::push_wait(size);
        let mut taken = 0;
        let mut last = String::new();
        for to in &urls {
            if taken >= wanted {
                break;
            }
            let mut path = format!("/silo/peer/uploads/{bucket}/{upload_id}");
            if let Some(n) = part {
                path.push_str(&format!("/{n}"));
            }
            match peers.post(to, &path, wait).await {
                Ok(()) => taken += 1,
                Err(e) => last = format!("{}: {e}", to.0),
            }
        }
        if taken >= wanted {
            return Ok(());
        }
        let what = match part {
            Some(n) => format!("part {n} of upload '{upload_id}'"),
            None => format!("upload '{upload_id}'"),
        };
        tracing::warn!(%bucket, upload_id, ?part, %last, "silo: no second copy of a staged upload");
        Err(ApiError::unavailable(format!(
            "{what} is stored on this server only: {taken} of {} other server(s) took a copy (last: {last}). \
             Send it again",
            urls.len()
        )))
    }
}

/// Take what the other servers stage and this one lacks, and remove
/// uploads they saw finish. A client with no peers does nothing.
pub async fn sync(client: &Client) -> ApiResult<StagedReport> {
    let mut report = StagedReport::default();
    let Some(peers) = client.peers() else {
        return Ok(report);
    };
    for from in peers.targets() {
        let base = &from.0;
        let list = match peers.get(&from, "/silo/peer/uploads").await {
            Ok(resp) => match resp.json::<Staged>().await {
                Ok(list) => list,
                Err(e) => {
                    tracing::debug!(peer = %base, error = %e, "silo: unreadable staging list");
                    continue;
                }
            },
            Err(e) => {
                tracing::debug!(peer = %base, error = %e, "silo: peer did not list its staging");
                continue;
            }
        };
        for done in &list.done {
            let Some(bucket) = Id::parse(crate::types::BucketSpec::KIND, &done.bucket) else {
                continue;
            };
            if client.store().collection::<Bucket>().get(&bucket).is_err()
                || client.upload_is_done(&bucket, &done.upload_id).await
            {
                continue;
            }
            let Ok(dir) = client.upload_dir(&bucket, &done.upload_id) else {
                continue;
            };
            // The mark first: a pass that stops here never takes it back.
            if client
                .mark_upload_done(&bucket, &done.upload_id)
                .await
                .is_err()
            {
                continue;
            }
            if tokio::fs::remove_dir_all(&dir).await.is_ok() {
                report.removed += 1;
            }
        }
        for upload in &list.uploads {
            match client.take(&from, upload, Want::All).await {
                Ok(n) => report.fetched += n,
                Err(e) => tracing::debug!(
                    peer = %base,
                    upload_id = %upload.upload_id,
                    error = %e,
                    "silo: could not copy a staged upload"
                ),
            }
        }
    }
    Ok(report)
}

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

const DONE_DIR: &str = ".uploads-done";

/// A peer that sends nothing for this long is given up on.
const STALL: Duration = Duration::from_secs(30);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

async fn modified_ms(path: &Path) -> Option<u64> {
    let modified = tokio::fs::metadata(path).await.ok()?.modified().ok()?;
    Some(modified.duration_since(UNIX_EPOCH).ok()?.as_millis() as u64)
}

/// The parts in a staging directory that have their MD5 beside them.
async fn read_parts(dir: &Path) -> Vec<StagedPart> {
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

async fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    tokio::fs::write(&tmp, bytes).await?;
    tokio::fs::rename(&tmp, path).await
}

/// Stream a part to a temp file beside `dest` and rename it only when its
/// length and MD5 are the ones the list gave. The file keeps the time the
/// part was written on the server it came from.
async fn download(
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
