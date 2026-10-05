// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Multipart upload staging under `{root}/{bucket}/.uploads/{uploadId}/`.
//!
//! Each staging dir holds `meta.json` (bucket, key, content type, initiation
//! time) plus `part-N` / `part-N.md5`. Abort and complete remove the dir;
//! uploads a client never finishes are removed by [`crate::gc`] once they
//! are older than `[modules.silo] multipart_ttl_secs`.
//!
//! Staging is plain files on disk, never store records, so an unfinished
//! upload is not an object: it does not show up in object listings or
//! object sizes.

use crate::client::Client;
use crate::types::*;
use bytes::Bytes;
use chrono::{DateTime, Utc};
use futures::Stream;
use md5::Md5;
use pistack_types::{ApiError, ApiResult, Id};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// Default minimum size of every part except the last (5 MiB, S3 rule).
pub const MIN_PART_BYTES: usize = 5 * 1024 * 1024;

/// S3 allows part numbers 1..=10000.
pub const MAX_PART_NUMBER: u32 = 10_000;

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UploadMeta {
    pub bucket: String,
    pub key: String,
    pub content_type: String,
    /// When CreateMultipartUpload ran. Absent on uploads staged before this
    /// field existed; readers fall back to the `meta.json` mtime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub initiated: Option<DateTime<Utc>>,
}

/// One in-progress upload, as returned by ListMultipartUploads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadSummary {
    pub key: String,
    pub upload_id: String,
    pub initiated: DateTime<Utc>,
}

/// A staging directory found on disk. `meta` is `None` when `meta.json` is
/// missing or unreadable (for example a create that crashed half way).
#[derive(Clone, Debug)]
pub(crate) struct StagedUpload {
    pub upload_id: String,
    pub dir: PathBuf,
    pub meta: Option<UploadMeta>,
    /// `meta.initiated`, else the `meta.json` mtime, else the dir mtime.
    pub initiated: Option<DateTime<Utc>>,
}

#[derive(Clone, Debug)]
pub struct PartInfo {
    pub part_number: u32,
    pub size: u64,
    pub etag: String,
}

#[derive(Clone, Debug)]
pub struct CompletedPart {
    pub part_number: u32,
    pub etag: String,
}

impl Client {
    fn uploads_root(&self, bucket: &Id) -> PathBuf {
        self.root().join(bucket.to_string()).join(".uploads")
    }

    fn upload_dir(&self, bucket: &Id, upload_id: &str) -> ApiResult<PathBuf> {
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
        E: std::fmt::Display,
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
            body,
            self.max_object_bytes(),
            content_length,
            expected_sha256,
        )
        .await?;
        let etag_path = dir.join(format!("part-{part_number}.md5"));
        tokio::fs::write(&etag_path, written.md5.as_bytes())
            .await
            .map_err(io_err)?;
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
    /// is ignored, as in S3. Returns `(page, truncated)`.
    pub async fn list_multipart_uploads(
        &self,
        bucket_id: &Id,
        prefix: Option<&str>,
        key_marker: Option<&str>,
        upload_id_marker: Option<&str>,
        max_uploads: usize,
    ) -> ApiResult<(Vec<UploadSummary>, bool)> {
        let _bucket = self.store().collection::<Bucket>().get(bucket_id)?;
        let bucket = bucket_id.to_string();
        let prefix = prefix.filter(|p| !p.is_empty());
        let key_marker = key_marker.filter(|m| !m.is_empty());
        let upload_id_marker = upload_id_marker.filter(|m| !m.is_empty());

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
            .filter(|u| prefix.is_none_or(|p| u.key.starts_with(p)))
            .filter(|u| match (key_marker, upload_id_marker) {
                (None, _) => true,
                (Some(km), None) => u.key.as_str() > km,
                (Some(km), Some(um)) => {
                    u.key.as_str() > km || (u.key == km && u.upload_id.as_str() > um)
                }
            })
            .collect();
        uploads.sort_by(|a, b| a.key.cmp(&b.key).then(a.upload_id.cmp(&b.upload_id)));
        let truncated = uploads.len() > max_uploads;
        uploads.truncate(max_uploads);
        Ok((uploads, truncated))
    }

    async fn load_upload_meta(&self, bucket_id: &Id, upload_id: &str) -> ApiResult<UploadMeta> {
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

    async fn remove_upload_dir(&self, bucket_id: &Id, upload_id: &str) -> ApiResult<()> {
        let dir = self.upload_dir(bucket_id, upload_id)?;
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(io_err(e)),
        }
    }
}

/// Every staging dir directly under `uploads_root`. A missing root is an
/// empty list; entries that are not directories or have an invalid upload
/// id are skipped.
pub(crate) async fn scan_staged(uploads_root: &Path) -> ApiResult<Vec<StagedUpload>> {
    let mut entries = match tokio::fs::read_dir(uploads_root).await {
        Ok(e) => e,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(io_err(e)),
    };
    let mut out = Vec::new();
    while let Some(entry) = entries.next_entry().await.map_err(io_err)? {
        let Ok(upload_id) = entry.file_name().into_string() else {
            continue;
        };
        if validate_upload_id(&upload_id).is_err() {
            continue;
        }
        let Ok(file_type) = entry.file_type().await else {
            continue;
        };
        if !file_type.is_dir() {
            continue;
        }
        out.push(read_staged(entry.path(), upload_id).await);
    }
    Ok(out)
}

async fn read_staged(dir: PathBuf, upload_id: String) -> StagedUpload {
    let meta_path = dir.join("meta.json");
    let meta = match tokio::fs::read(&meta_path).await {
        Ok(bytes) => match serde_json::from_slice::<UploadMeta>(&bytes) {
            Ok(m) => Some(m),
            Err(e) => {
                tracing::debug!(path = %meta_path.display(), error = %e, "silo: unreadable upload meta");
                None
            }
        },
        Err(_) => None,
    };
    let initiated = match meta.as_ref().and_then(|m| m.initiated) {
        Some(t) => Some(t),
        None => match mtime(&meta_path).await {
            Some(t) => Some(t),
            None => mtime(&dir).await,
        },
    };
    StagedUpload {
        upload_id,
        dir,
        meta,
        initiated,
    }
}

async fn mtime(path: &Path) -> Option<DateTime<Utc>> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    meta.modified().ok().map(DateTime::<Utc>::from)
}

/// Parse CompleteMultipartUpload XML into ordered parts.
pub fn parse_complete_xml(body: &str) -> ApiResult<Vec<CompletedPart>> {
    // Minimal tag walk — avoids a XML crate dependency for this one shape.
    let mut parts = Vec::new();
    let mut rest = body;
    while let Some(start) = rest.find("<Part>") {
        let after = &rest[start + 6..];
        let Some(end) = after.find("</Part>") else {
            return Err(s3_err("MalformedXML", "unclosed <Part>"));
        };
        let block = &after[..end];
        let number = xml_text(block, "PartNumber")?
            .parse::<u32>()
            .map_err(|_| s3_err("MalformedXML", "PartNumber must be a number"))?;
        let etag = xml_text(block, "ETag")?;
        parts.push(CompletedPart {
            part_number: number,
            etag,
        });
        rest = &after[end + 7..];
    }
    Ok(parts)
}

fn xml_text(block: &str, tag: &str) -> ApiResult<String> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let Some(s) = block.find(&open) else {
        return Err(s3_err(
            "MalformedXML",
            format!("missing <{tag}> in CompleteMultipartUpload"),
        ));
    };
    let after = &block[s + open.len()..];
    let Some(e) = after.find(&close) else {
        return Err(s3_err(
            "MalformedXML",
            format!("unclosed <{tag}> in CompleteMultipartUpload"),
        ));
    };
    Ok(after[..e].trim().to_owned())
}

fn normalize_etag(raw: &str) -> String {
    raw.trim()
        .trim_matches('"')
        .trim_start_matches("&quot;")
        .trim_end_matches("&quot;")
        .to_ascii_lowercase()
}

fn validate_upload_id(id: &str) -> ApiResult<()> {
    if id.is_empty() || id.len() > 128 || id.contains('/') || id.contains('\\') || id.contains("..")
    {
        return Err(s3_err("NoSuchUpload", "invalid upload id"));
    }
    Ok(())
}

/// Attach an S3 error code in `details.s3_code` for the HTTP layer.
pub fn s3_err(code: &str, message: impl Into<String>) -> ApiError {
    let message = message.into();
    let status_code = match code {
        "NoSuchUpload" | "NoSuchKey" | "NoSuchBucket" => ApiError::not_found(message),
        "EntityTooLarge"
        | "EntityTooSmall"
        | "InvalidPart"
        | "InvalidPartOrder"
        | "InvalidArgument"
        | "InvalidRequest"
        | "MalformedXML"
        | "AuthorizationHeaderMalformed"
        | "XAmzContentSHA256Mismatch" => ApiError::bad_request(message),
        "InvalidAccessKeyId"
        | "SignatureDoesNotMatch"
        | "AccessDenied"
        | "RequestTimeTooSkewed" => ApiError::forbidden(message),
        "NotImplemented" => ApiError::not_implemented(message),
        _ => ApiError::internal(message),
    };
    status_code.with_detail("s3_code", code)
}

pub fn s3_code_of(err: &ApiError) -> Option<&str> {
    err.details.get("s3_code").and_then(|v| v.as_str())
}

pub(crate) fn io_err(e: std::io::Error) -> ApiError {
    ApiError::internal(format!("silo: {e}"))
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        s.push(HEX[(b >> 4) as usize] as char);
        s.push(HEX[(b & 0xf) as usize] as char);
    }
    s
}

fn hex_decode_md5(hex: &str) -> ApiResult<[u8; 16]> {
    let hex = normalize_etag(hex);
    if hex.len() != 32 || !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(s3_err("InvalidPart", "ETag is not a 32-char hex MD5"));
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
            .map_err(|_| s3_err("InvalidPart", "ETag is not a 32-char hex MD5"))?;
    }
    Ok(out)
}

struct PartWritten {
    md5: String,
}

async fn write_part_atomic<S, E>(
    dest: &Path,
    body: S,
    max: usize,
    content_length: Option<u64>,
    expected_sha256: Option<&str>,
) -> ApiResult<PartWritten>
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: std::fmt::Display,
{
    use futures::StreamExt;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
    }
    let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(io_err)?;
        let mut md5 = Md5::new();
        let mut sha = Sha256::new();
        let mut size: u64 = 0;
        let mut body = std::pin::pin!(body);
        while let Some(chunk) = body.next().await {
            let chunk = chunk.map_err(|e| ApiError::bad_request(format!("reading body: {e}")))?;
            let next = size.saturating_add(chunk.len() as u64);
            if next > max as u64 {
                return Err(s3_err("EntityTooLarge", object_too_large_message(max)));
            }
            if let Some(len) = content_length {
                if next > len {
                    return Err(ApiError::bad_request(
                        "body longer than Content-Length".to_owned(),
                    ));
                }
            }
            md5.update(&chunk);
            sha.update(&chunk);
            file.write_all(&chunk).await.map_err(io_err)?;
            size = next;
        }
        if let Some(len) = content_length {
            if size != len {
                return Err(ApiError::bad_request(format!(
                    "body length {size} does not match Content-Length {len}"
                )));
            }
        }
        file.flush().await.map_err(io_err)?;
        file.sync_all().await.map_err(io_err)?;
        drop(file);
        let sha256 = hex_encode(sha.finalize().as_slice());
        if let Some(want) = expected_sha256 {
            let want = want.to_ascii_lowercase();
            if !bool::from(sha256.as_bytes().ct_eq(want.as_bytes())) {
                return Err(s3_err(
                    "XAmzContentSHA256Mismatch",
                    "The provided 'x-amz-content-sha256' header does not match what was computed.",
                ));
            }
        }
        tokio::fs::rename(&tmp, dest).await.map_err(io_err)?;
        Ok(PartWritten {
            md5: hex_encode(md5.finalize().as_slice()),
        })
    }
    .await;
    if result.is_err() {
        // Discard the temp file only. A prior successful part at `dest`
        // stays until a later UploadPart renames over it.
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

struct ConcatState {
    parts: std::vec::IntoIter<PathBuf>,
    current: Option<tokio::fs::File>,
    buf: Vec<u8>,
}

fn concat_parts_stream(
    parts: Vec<(PathBuf, u64)>,
) -> impl Stream<Item = Result<Bytes, std::io::Error>> + Send {
    let state = ConcatState {
        parts: parts
            .into_iter()
            .map(|(p, _)| p)
            .collect::<Vec<_>>()
            .into_iter(),
        current: None,
        buf: vec![0u8; 256 * 1024],
    };
    futures::stream::unfold(state, |mut st| async move {
        loop {
            if st.current.is_none() {
                let path = st.parts.next()?;
                match tokio::fs::File::open(&path).await {
                    Ok(f) => st.current = Some(f),
                    Err(e) => return Some((Err(e), st)),
                }
            }
            let file = st.current.as_mut().unwrap();
            match file.read(&mut st.buf).await {
                Ok(0) => {
                    st.current = None;
                    continue;
                }
                Ok(n) => {
                    let bytes = Bytes::copy_from_slice(&st.buf[..n]);
                    return Some((Ok(bytes), st));
                }
                Err(e) => return Some((Err(e), st)),
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_complete_parts() {
        let xml = r#"<?xml version="1.0"?>
<CompleteMultipartUpload>
  <Part><PartNumber>1</PartNumber><ETag>"aabb"</ETag></Part>
  <Part><PartNumber>2</PartNumber><ETag>ccdd</ETag></Part>
</CompleteMultipartUpload>"#;
        let parts = parse_complete_xml(xml).unwrap();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0].part_number, 1);
        assert_eq!(normalize_etag(&parts[0].etag), "aabb");
        assert_eq!(parts[1].part_number, 2);
    }
}
