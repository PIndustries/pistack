// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::multipart::{io_err, scan_staged};
use crate::types::Bucket;
use pistack_types::{ApiError, ApiResult, Id, Spec};
use std::path::PathBuf;

use super::done_dir::DONE_DIR;
use super::done_upload::DoneUpload;
use super::download::download;
use super::read_parts::read_parts;
use super::staged::Staged;
use super::staged_part::StagedPart;
use super::staged_upload::StagedUpload;
use super::want::Want;
use super::write_atomic::write_atomic;

impl Client {
    pub(crate) fn done_dir(&self, bucket: &Id) -> PathBuf {
        self.root().join(bucket.to_string()).join(DONE_DIR)
    }

    pub(crate) fn done_path(&self, bucket: &Id, upload_id: &str) -> ApiResult<PathBuf> {
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

    pub(crate) async fn upload_is_done(&self, bucket: &Id, upload_id: &str) -> bool {
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

    /// Take from the peer at `base` the files of `upload` this server
    /// lacks. Returns how many parts were fetched.
    pub(crate) async fn take(
        &self,
        base: &str,
        upload: &StagedUpload,
        want: Want,
    ) -> Result<usize, String> {
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
            let dest = dir.join(format!("part-{}", part.number));
            let resp = crate::staging_fetch::fetch_part(
                self,
                base,
                &upload.bucket,
                &upload.upload_id,
                part.number,
            )
            .await?;
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
        for base in peers.urls() {
            let name = bucket.to_string();
            let found = crate::staging_fetch::fetch_upload(self, &base, &name, upload_id);
            let upload = match found.await {
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
            if let Err(e) = self.take(&base, &upload, want).await {
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

    pub(crate) async fn holds(
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
        let urls = peers.urls();
        let wanted = (self.put_copies().saturating_sub(1) as usize).min(urls.len());
        if wanted == 0 {
            return Ok(());
        }
        let wait = crate::replica::push_wait(size);
        let mut taken = 0;
        let mut last = String::new();
        for base in &urls {
            if taken >= wanted {
                break;
            }
            match crate::staging_fetch::ask_peer(
                self,
                base,
                &bucket.to_string(),
                upload_id,
                part,
                wait,
            )
            .await
            {
                Ok(()) => taken += 1,
                Err(e) => last = format!("{base}: {e}"),
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
