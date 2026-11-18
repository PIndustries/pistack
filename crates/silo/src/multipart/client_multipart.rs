// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::client::Client;
use crate::types::*;
use bytes::Bytes;
use chrono::Utc;
use futures::Stream;
use md5::Md5;
use pistack_types::{ApiError, ApiResult, Id};
use sha2::Digest;
use std::path::PathBuf;

use super::completed_part::CompletedPart;
use super::concat_parts_stream::concat_parts_stream;
use super::hex_decode_md5::hex_decode_md5;
use super::hex_encode::hex_encode;
use super::io_err::io_err;
use super::list_upload_entry::ListUploadEntry;
use super::max_part_number::MAX_PART_NUMBER;
use super::normalize_etag::normalize_etag;
use super::part_info::PartInfo;
use super::s3_err::s3_err;
use super::scan_staged::scan_staged;
use super::upload_meta::UploadMeta;
use super::upload_summary::UploadSummary;
use super::validate_upload_id::validate_upload_id;
use super::write_part_atomic::write_part_atomic;

impl Client {
    pub(crate) fn uploads_root(&self, bucket: &Id) -> PathBuf {
        self.root().join(bucket.to_string()).join(".uploads")
    }

    pub(crate) fn upload_dir(&self, bucket: &Id, upload_id: &str) -> ApiResult<PathBuf> {
        validate_upload_id(upload_id)?;
        Ok(self.uploads_root(bucket).join(upload_id))
    }

    /// CreateMultipartUpload: allocate an upload id and staging directory.
    pub async fn create_multipart_upload(
        &self,
        bucket_id: &Id,
        key: &str,
        content_type: Option<String>,
    ) -> ApiResult<String> {
        validate_key(key)?;
        let _bucket = self.store().collection::<Bucket>().get(bucket_id)?;
        let upload_id = uuid::Uuid::now_v7().to_string();
        let dir = self.upload_dir(bucket_id, &upload_id)?;
        tokio::fs::create_dir_all(&dir).await.map_err(io_err)?;
        let meta = UploadMeta {
            bucket: bucket_id.to_string(),
            key: key.to_owned(),
            content_type: content_type_or_default(content_type),
            initiated: Some(Utc::now()),
        };
        let meta_path = dir.join("meta.json");
        let bytes = serde_json::to_vec_pretty(&meta)
            .map_err(|e| ApiError::internal(format!("silo multipart meta: {e}")))?;
        tokio::fs::write(&meta_path, bytes).await.map_err(io_err)?;
        self.push_staged(bucket_id, &upload_id, None, 0).await?;
        Ok(upload_id)
    }

    /// UploadPart: stream a part body to disk; return the part MD5 ETag (hex).
    ///
    /// When `expected_sha256` is set (lowercase hex from SigV4
    /// `x-amz-content-sha256`), the streamed body must match or the staged
    /// part file is discarded and `XAmzContentSHA256Mismatch` is returned.
    /// `None` means `UNSIGNED-PAYLOAD` (or non-SigV4) — no payload check.
    #[allow(clippy::too_many_arguments)]
    pub async fn upload_part<S, E>(
        &self,
        bucket_id: &Id,
        key: &str,
        upload_id: &str,
        part_number: u32,
        body: S,
        content_length: Option<u64>,
        expected_sha256: Option<&str>,
    ) -> ApiResult<String>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
        E: crate::client::IntoBodyReadErr + Send + 'static,
    {
        if !(1..=MAX_PART_NUMBER).contains(&part_number) {
            return Err(s3_err(
                "InvalidArgument",
                format!("partNumber must be 1..={MAX_PART_NUMBER}"),
            ));
        }
        let meta = self.load_upload_meta(bucket_id, upload_id).await?;
        if meta.key != key {
            return Err(s3_err(
                "NoSuchUpload",
                format!("upload '{upload_id}' is for a different key"),
            ));
        }
        if let Some(len) = content_length {
            if len > self.max_object_bytes() as u64 {
                return Err(s3_err(
                    "EntityTooLarge",
                    object_too_large_message(self.max_object_bytes()),
                ));
            }
        }
        let dir = self.upload_dir(bucket_id, upload_id)?;
        let part_path = dir.join(format!("part-{part_number}"));
        let written = write_part_atomic(
            &part_path,
            crate::client::map_stream_err(body),
            self.max_object_bytes(),
            content_length,
            expected_sha256,
        )
        .await?;
        let etag_path = dir.join(format!("part-{part_number}.md5"));
        tokio::fs::write(&etag_path, written.md5.as_bytes())
            .await
            .map_err(io_err)?;
        let size = tokio::fs::metadata(&part_path)
            .await
            .map(|m| m.len())
            .unwrap_or(0);
        self.push_staged(bucket_id, upload_id, Some(part_number), size)
            .await?;
        Ok(written.md5)
    }

    /// ListParts for an in-progress upload.
    pub async fn list_parts(
        &self,
        bucket_id: &Id,
        key: &str,
        upload_id: &str,
    ) -> ApiResult<(UploadMeta, Vec<PartInfo>)> {
        let meta = self.load_upload_meta(bucket_id, upload_id).await?;
        if meta.key != key {
            return Err(s3_err(
                "NoSuchUpload",
                format!("upload '{upload_id}' is for a different key"),
            ));
        }
        let dir = self.upload_dir(bucket_id, upload_id)?;
        let mut parts = Vec::new();
        let mut entries = tokio::fs::read_dir(&dir).await.map_err(io_err)?;
        while let Some(entry) = entries.next_entry().await.map_err(io_err)? {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            let Some(num) = name.strip_prefix("part-") else {
                continue;
            };
            if name.ends_with(".md5") {
                continue;
            }
            let Ok(part_number) = num.parse::<u32>() else {
                continue;
            };
            let etag = tokio::fs::read_to_string(dir.join(format!("part-{part_number}.md5")))
                .await
                .map_err(|_| {
                    s3_err(
                        "InternalError",
                        format!("part {part_number} missing md5 sidecar"),
                    )
                })?;
            let meta = entry.metadata().await.map_err(io_err)?;
            parts.push(PartInfo {
                part_number,
                size: meta.len(),
                etag: etag.trim().to_owned(),
            });
        }
        parts.sort_by_key(|p| p.part_number);
        Ok((meta, parts))
    }

    /// CompleteMultipartUpload: verify parts, concatenate, atomic rename.
    pub async fn complete_multipart_upload(
        &self,
        bucket_id: &Id,
        key: &str,
        upload_id: &str,
        parts: &[CompletedPart],
    ) -> ApiResult<(crate::types::Object, String)> {
        let meta = self.load_upload_meta(bucket_id, upload_id).await?;
        if meta.key != key {
            return Err(s3_err(
                "NoSuchUpload",
                format!("upload '{upload_id}' is for a different key"),
            ));
        }
        if parts.is_empty() {
            return Err(s3_err(
                "InvalidRequest",
                "CompleteMultipartUpload requires at least one part",
            ));
        }
        // Ascending part numbers, no duplicates.
        for w in parts.windows(2) {
            if w[0].part_number >= w[1].part_number {
                return Err(s3_err(
                    "InvalidPartOrder",
                    "part numbers must be strictly ascending",
                ));
            }
        }

        let dir = self.upload_dir(bucket_id, upload_id)?;
        let mut binary_md5s: Vec<u8> = Vec::with_capacity(parts.len() * 16);
        let mut total: u64 = 0;
        let mut part_paths: Vec<(PathBuf, u64)> = Vec::with_capacity(parts.len());

        for (i, part) in parts.iter().enumerate() {
            let last = i + 1 == parts.len();
            let path = dir.join(format!("part-{}", part.part_number));
            let etag_path = dir.join(format!("part-{}.md5", part.part_number));
            let stored = match tokio::fs::read_to_string(&etag_path).await {
                Ok(s) => s.trim().to_owned(),
                Err(_) => {
                    return Err(s3_err(
                        "InvalidPart",
                        format!("part {} was not uploaded", part.part_number),
                    ));
                }
            };
            let want = normalize_etag(&part.etag);
            if stored != want {
                return Err(s3_err(
                    "InvalidPart",
                    format!(
                        "part {} ETag mismatch: got {}, expected {}",
                        part.part_number, want, stored
                    ),
                ));
            }
            let file_meta = match tokio::fs::metadata(&path).await {
                Ok(m) => m,
                Err(_) => {
                    return Err(s3_err(
                        "InvalidPart",
                        format!("part {} was not uploaded", part.part_number),
                    ));
                }
            };
            let size = file_meta.len();
            if !last && size < self.min_part_bytes() as u64 {
                return Err(s3_err(
                    "EntityTooSmall",
                    format!(
                        "part {} is {size} bytes; minimum is {} except the last part",
                        part.part_number,
                        self.min_part_bytes()
                    ),
                ));
            }
            total = total.saturating_add(size);
            if total > self.max_object_bytes() as u64 {
                return Err(s3_err(
                    "EntityTooLarge",
                    object_too_large_message(self.max_object_bytes()),
                ));
            }
            let digest = hex_decode_md5(&stored)?;
            binary_md5s.extend_from_slice(&digest);
            part_paths.push((path, size));
        }

        let multipart_etag = {
            let digest = Md5::digest(&binary_md5s);
            format!("{}-{}", hex_encode(digest.as_slice()), parts.len())
        };

        // Concatenate into the final object via the normal put path.
        let stream = concat_parts_stream(part_paths);
        let obj = self
            .put_object_stream(
                bucket_id,
                key,
                stream,
                Some(total),
                Some(meta.content_type.clone()),
            )
            .await
            .map_err(|e| {
                // Map generic oversize to EntityTooLarge for S3 clients.
                if e.message.contains("larger than") {
                    s3_err("EntityTooLarge", e.message)
                } else {
                    e
                }
            })?;

        // Overwrite status.md5 with the multipart ETag form (hex-N).
        let mut obj = obj;
        obj.status.md5 = multipart_etag.clone();
        let obj = self
            .store()
            .collection::<crate::types::Object>()
            .update(obj, None)
            .map_err(ApiError::from)?;

        self.remove_upload_dir(bucket_id, upload_id).await?;
        Ok((obj, multipart_etag))
    }

    /// AbortMultipartUpload: delete staging data.
    pub async fn abort_multipart_upload(
        &self,
        bucket_id: &Id,
        key: &str,
        upload_id: &str,
    ) -> ApiResult<()> {
        let meta = self.load_upload_meta(bucket_id, upload_id).await?;
        if meta.key != key {
            return Err(s3_err(
                "NoSuchUpload",
                format!("upload '{upload_id}' is for a different key"),
            ));
        }
        self.remove_upload_dir(bucket_id, upload_id).await
    }

    /// ListMultipartUploads: in-progress uploads in the bucket, ordered by
    /// key then upload id (upload ids are time-ordered, so this is
    /// initiation order within a key).
    ///
    /// `key_marker` skips keys up to and including it; with
    /// `upload_id_marker` as well, uploads for `key_marker` itself with a
    /// larger upload id are kept. `upload_id_marker` without `key_marker`
    /// is ignored, as in S3.
    ///
    /// Non-empty `delimiter` rolls keys into [`ListUploadEntry::CommonPrefix`]
    /// (each counts once toward `max_uploads`). Returns `(page, truncated)`.
    pub async fn list_multipart_uploads(
        &self,
        bucket_id: &Id,
        prefix: Option<&str>,
        key_marker: Option<&str>,
        upload_id_marker: Option<&str>,
        max_uploads: usize,
        delimiter: Option<&str>,
    ) -> ApiResult<(Vec<ListUploadEntry>, bool)> {
        let _bucket = self.store().collection::<Bucket>().get(bucket_id)?;
        let bucket = bucket_id.to_string();
        let prefix = prefix.unwrap_or("").to_owned();
        let prefix_filter = if prefix.is_empty() {
            None
        } else {
            Some(prefix.as_str())
        };
        let key_marker = key_marker.filter(|m| !m.is_empty());
        // S3 ignores upload-id-marker unless key-marker is set.
        let upload_id_marker = if key_marker.is_some() {
            upload_id_marker.filter(|m| !m.is_empty())
        } else {
            None
        };
        let delimiter = delimiter.filter(|d| !d.is_empty());

        let mut uploads: Vec<UploadSummary> = scan_staged(&self.uploads_root(bucket_id))
            .await?
            .into_iter()
            .filter_map(|st| {
                let meta = st.meta?;
                if meta.bucket != bucket {
                    return None;
                }
                Some(UploadSummary {
                    key: meta.key,
                    upload_id: st.upload_id,
                    initiated: st.initiated?,
                })
            })
            .filter(|u| prefix_filter.is_none_or(|p| u.key.starts_with(p)))
            .filter(|u| {
                crate::delimiter::upload_after_markers(
                    &u.key,
                    &u.upload_id,
                    key_marker,
                    upload_id_marker,
                    delimiter,
                )
            })
            .collect();
        uploads.sort_by(|a, b| a.key.cmp(&b.key).then(a.upload_id.cmp(&b.upload_id)));

        let entries: Vec<ListUploadEntry> = match delimiter {
            Some(d) => {
                crate::delimiter::collapse_with_delimiter(uploads, &prefix, d, |u| u.key.as_str())
                    .into_iter()
                    .map(|e| match e {
                        crate::delimiter::Delimited::Item(u) => ListUploadEntry::Upload(u),
                        crate::delimiter::Delimited::CommonPrefix(cp) => {
                            ListUploadEntry::CommonPrefix(cp)
                        }
                    })
                    .collect()
            }
            None => uploads.into_iter().map(ListUploadEntry::Upload).collect(),
        };
        let truncated = entries.len() > max_uploads;
        let page = entries.into_iter().take(max_uploads).collect();
        Ok((page, truncated))
    }

    pub(crate) async fn load_upload_meta(
        &self,
        bucket_id: &Id,
        upload_id: &str,
    ) -> ApiResult<UploadMeta> {
        let dir = self.upload_dir(bucket_id, upload_id)?;
        let meta_path = dir.join("meta.json");
        let bytes = match tokio::fs::read(&meta_path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(s3_err(
                    "NoSuchUpload",
                    format!("upload '{upload_id}' does not exist"),
                ));
            }
            Err(e) => return Err(io_err(e)),
        };
        let meta: UploadMeta = serde_json::from_slice(&bytes)
            .map_err(|e| ApiError::internal(format!("silo multipart meta: {e}")))?;
        if meta.bucket != bucket_id.to_string() {
            return Err(s3_err(
                "NoSuchUpload",
                format!("upload '{upload_id}' does not exist"),
            ));
        }
        Ok(meta)
    }

    /// The upload is over. The mark is written first so no other server
    /// hands the staging back.
    pub(crate) async fn remove_upload_dir(&self, bucket_id: &Id, upload_id: &str) -> ApiResult<()> {
        let dir = self.upload_dir(bucket_id, upload_id)?;
        self.mark_upload_done(bucket_id, upload_id).await?;
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }
}
