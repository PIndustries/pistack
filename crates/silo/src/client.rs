// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bucket and object writes.
//!
//! Bytes for the current generation of a key live at
//! `{root}/{bucket-id}/{key}`, where `root` is `$PISTACK_DATA_DIR/silo` in
//! the module. `status.sha256` is the content address of those bytes;
//! `status.md5` is the S3 single-PUT ETag. With bucket versioning, the
//! previous generation's bytes are kept at
//! `{root}/{bucket-id}/.blobs/{sha256}` and the old record stays with
//! `status.latest = false`.
//!
//! Puts stream the request body to a temp file (hashing as they go), then
//! rename into place. Gets can stream the file back without buffering it.
//!
//! A put lands on one server's disk. With [`Client::with_peers`], a read
//! on a server that has the record but not the bytes fetches them from
//! another server first; see [`crate::replica`].

use crate::types::*;
use bytes::Bytes;
use futures::Stream;
use md5::Md5;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id, Meta, Phase, Spec};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use subtle::ConstantTimeEq;
use tokio::io::{AsyncReadExt, AsyncWriteExt};

#[derive(Clone)]
pub struct Client {
    store: Store,
    root: PathBuf,
    max_object_bytes: usize,
    min_part_bytes: usize,
    /// Other servers to ask for bytes this one lacks.
    peers: Option<crate::replica::Peers>,
    /// Servers that must hold an object's bytes before a put is answered.
    put_copies: u32,
}

impl Client {
    pub fn new(store: Store, root: impl Into<PathBuf>) -> Self {
        Self {
            store,
            root: root.into(),
            max_object_bytes: MAX_OBJECT_BYTES,
            min_part_bytes: MIN_PART_BYTES,
            peers: None,
            put_copies: crate::replica::PUT_COPIES,
        }
    }

    /// How many servers must hold an object's bytes before a put is
    /// answered: `1` is this server alone, `2` (the default) waits for one
    /// peer to take a copy. It has no effect on a client with no peers, or
    /// in a cluster of one server.
    pub fn with_put_copies(mut self, copies: u32) -> Self {
        self.put_copies = copies.max(1);
        self
    }

    /// Fetch bytes this server lacks from `peers` when an object is read.
    pub fn with_peers(mut self, peers: crate::replica::Peers) -> Self {
        self.peers = Some(peers);
        self
    }

    pub(crate) fn put_copies(&self) -> u32 {
        self.put_copies
    }

    pub fn peers(&self) -> Option<&crate::replica::Peers> {
        self.peers.as_ref()
    }

    /// Cap for a single PUT. Used by HTTP and by [`Self::put_object`].
    pub fn with_max_object_bytes(mut self, max: usize) -> Self {
        self.max_object_bytes = max;
        self
    }

    pub fn max_object_bytes(&self) -> usize {
        self.max_object_bytes
    }

    /// Minimum size of every multipart part except the last (default 5 MiB).
    pub fn with_min_part_bytes(mut self, min: usize) -> Self {
        self.min_part_bytes = min;
        self
    }

    pub fn min_part_bytes(&self) -> usize {
        self.min_part_bytes
    }

    pub fn store(&self) -> &Store {
        &self.store
    }

    /// Directory that holds `{bucket-id}/{key}`.
    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create_bucket(&self, mut meta: Meta, spec: BucketSpec) -> ApiResult<Bucket> {
        meta.name = clean_name(&meta.name)?;
        if meta.id.kind != BucketSpec::KIND {
            meta.id = Id::new(BucketSpec::KIND);
        }
        let bucket = Bucket {
            meta,
            spec,
            status: BucketStatus {
                phase: Phase::Ready,
            },
        };
        let project = bucket.meta.project.clone();
        crate::usage::admitted(&self.store, project.as_ref(), || {
            self.store
                .collection::<Bucket>()
                .create(bucket)
                .map_err(Into::into)
        })
    }

    pub fn get_bucket(&self, project: Option<&Id>, name_or_id: &str) -> ApiResult<Bucket> {
        self.store
            .collection::<Bucket>()
            .resolve(project, name_or_id)
            .map_err(Into::into)
    }

    pub fn list_buckets(&self, project: Option<&Id>) -> ApiResult<Vec<Bucket>> {
        self.store
            .collection::<Bucket>()
            .list(project)
            .map_err(Into::into)
    }

    /// Delete the bucket, its object records, and the bucket directory.
    pub async fn delete_bucket(&self, id: &Id) -> ApiResult<Bucket> {
        let bucket = self.store.collection::<Bucket>().get(id)?;
        let dir = self.bucket_dir(&bucket.meta.id);
        match tokio::fs::remove_dir_all(&dir).await {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_err(e)),
        }
        for obj in self.objects_for_bucket(&bucket)? {
            self.store.collection::<Object>().delete(&obj.meta.id)?;
        }
        let bucket = self.store.collection::<Bucket>().delete(id)?;
        crate::usage::released(&self.store, bucket.meta.project.as_ref());
        Ok(bucket)
    }

    /// Path of the current bytes for `key`, after the key is checked.
    pub fn key_path(&self, bucket: &Id, key: &str) -> ApiResult<PathBuf> {
        validate_key(key)?;
        let dir = self.bucket_dir(bucket);
        let path = dir.join(key);
        if !path.starts_with(&dir) {
            return Err(ApiError::bad_request("object key escapes the bucket"));
        }
        Ok(path)
    }

    /// Buffering convenience over [`Self::put_object_stream`].
    pub async fn put_object(
        &self,
        bucket_id: &Id,
        key: &str,
        bytes: &[u8],
        content_type: Option<String>,
    ) -> ApiResult<Object> {
        if bytes.len() > self.max_object_bytes {
            return Err(ApiError::bad_request(object_too_large_message(
                self.max_object_bytes,
            )));
        }
        let owned = Bytes::copy_from_slice(bytes);
        let stream = futures::stream::once(async move { Ok::<Bytes, std::io::Error>(owned) });
        self.put_object_stream(
            bucket_id,
            key,
            stream,
            Some(bytes.len() as u64),
            content_type,
        )
        .await
    }

    /// Stream `body` to a temp file under the bucket, hash while writing,
    /// then atomically rename onto `{bucket}/{key}`.
    pub async fn put_object_stream<S, E>(
        &self,
        bucket_id: &Id,
        key: &str,
        body: S,
        content_length: Option<u64>,
        content_type: Option<String>,
    ) -> ApiResult<Object>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
        E: IntoBodyReadErr + Send + 'static,
    {
        self.put_object_stream_sha(bucket_id, key, body, content_length, content_type, None)
            .await
    }

    /// [`put_object_stream`] with an optional declared payload SHA-256.
    ///
    /// When the cluster has another server, the put is answered only after
    /// one of them holds the bytes too (see [`Self::with_put_copies`]). If
    /// none takes a copy the answer is `unavailable`, and the object is
    /// still stored here: read before retrying.
    pub async fn put_object_stream_sha<S, E>(
        &self,
        bucket_id: &Id,
        key: &str,
        body: S,
        content_length: Option<u64>,
        content_type: Option<String>,
        expected_sha256: Option<&str>,
    ) -> ApiResult<Object>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
        E: IntoBodyReadErr + Send + 'static,
    {
        let obj = self
            .put_here(
                bucket_id,
                key,
                body,
                content_length,
                content_type,
                expected_sha256,
            )
            .await?;
        self.second_copy(&obj).await?;
        Ok(obj)
    }

    /// Wait until enough other servers hold `obj`'s bytes.
    async fn second_copy(&self, obj: &Object) -> ApiResult<()> {
        let Some(peers) = &self.peers else {
            return Ok(());
        };
        let wanted = self.put_copies.saturating_sub(1) as usize;
        if wanted == 0 {
            return Ok(());
        }
        peers.push(obj, wanted).await
    }

    /// Write the object on this server and record it.
    async fn put_here<S, E>(
        &self,
        bucket_id: &Id,
        key: &str,
        body: S,
        content_length: Option<u64>,
        content_type: Option<String>,
        expected_sha256: Option<&str>,
    ) -> ApiResult<Object>
    where
        S: Stream<Item = Result<Bytes, E>> + Send,
        E: IntoBodyReadErr + Send + 'static,
    {
        if let Some(len) = content_length {
            if len > self.max_object_bytes as u64 {
                return Err(ApiError::bad_request(object_too_large_message(
                    self.max_object_bytes,
                )));
            }
        }
        validate_key(key)?;
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let path = self.key_path(&bucket.meta.id, key)?;
        let existing = self.find_current(&bucket, key)?;
        let version = existing
            .as_ref()
            .map(|o| o.status.version.saturating_add(1))
            .unwrap_or(1);
        let ctype = match content_type {
            Some(v) if !v.trim().is_empty() => v,
            _ => existing
                .as_ref()
                .map(|o| o.spec.content_type.clone())
                .unwrap_or_else(|| content_type_or_default(None)),
        };

        if bucket.spec.versioning {
            if let Some(prev) = &existing {
                self.preserve_previous(&bucket.meta.id, &prev.status.sha256, &path)
                    .await?;
            }
        }

        let written = write_stream_atomic(
            &path,
            map_stream_err(body),
            self.max_object_bytes,
            content_length,
            expected_sha256,
        )
        .await?;
        // Before the record: a file with a marker is never taken for
        // another generation of the key, whatever happens to the write.
        crate::replica::write_marker(&self.root, &bucket.meta.id, key, &written.sha256)
            .await
            .map_err(io_err)?;

        if bucket.spec.versioning {
            if let Some(prev) = existing {
                let mut old = prev;
                old.status.latest = false;
                self.store.collection::<Object>().update(old, None)?;
            }
            return self.insert_object(
                &bucket,
                key,
                ctype,
                written.sha256,
                written.md5,
                written.size,
                version,
            );
        }

        if let Some(prev) = existing {
            let mut next = prev;
            next.spec.content_type = ctype;
            next.status.sha256 = written.sha256;
            next.status.md5 = written.md5;
            next.status.size_bytes = written.size;
            next.status.version = version;
            next.status.latest = true;
            return self
                .store
                .collection::<Object>()
                .update(next, None)
                .map_err(Into::into);
        }
        self.insert_object(
            &bucket,
            key,
            ctype,
            written.sha256,
            written.md5,
            written.size,
            version,
        )
    }

    /// Read the whole object into memory (callers that already buffer).
    pub async fn get_object(&self, bucket_id: &Id, key: &str) -> ApiResult<(Object, Vec<u8>)> {
        let (obj, mut file) = self.open_object(bucket_id, key).await?;
        let mut bytes = Vec::with_capacity(obj.status.size_bytes as usize);
        file.read_to_end(&mut bytes).await.map_err(io_err)?;
        let got = sha256_hex(&bytes);
        if got != obj.status.sha256 {
            return Err(ApiError::internal(format!(
                "object '{key}' sha256 mismatch: status {}, disk {got}",
                obj.status.sha256
            )));
        }
        Ok((obj, bytes))
    }

    /// Open the object file for streaming. Verifies size matches status;
    /// SHA-256 is checked by callers that buffer, or left to disk integrity.
    ///
    /// When this server does not hold the current bytes and the client has
    /// peers, they are fetched and checked first. `unavailable` when no
    /// peer returned them; `not_found` when there is nobody to ask.
    pub async fn open_object(
        &self,
        bucket_id: &Id,
        key: &str,
    ) -> ApiResult<(Object, tokio::fs::File)> {
        validate_key(key)?;
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let obj = self
            .find_current(&bucket, key)?
            .ok_or_else(|| ApiError::not_found(format!("object '{key}' not found")))?;
        let path = self.key_path(&bucket.meta.id, key)?;
        match self.ensure_bytes(&obj).await {
            Ok(_) => {}
            // Nobody to ask. The open below reports the missing file.
            Err(e) if e.code == pistack_types::error::ErrorCode::NotFound => {}
            Err(e) => return Err(e),
        }
        let file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApiError::not_found(format!("object '{key}' not found")));
            }
            Err(e) => return Err(io_err(e)),
        };
        let meta = file.metadata().await.map_err(io_err)?;
        if meta.len() != obj.status.size_bytes {
            return Err(ApiError::internal(format!(
                "object '{key}' size mismatch: status {}, disk {}",
                obj.status.size_bytes,
                meta.len()
            )));
        }
        Ok((obj, file))
    }

    /// Make sure this server holds the bytes of `obj`, fetching them from
    /// a peer when it does not. Returns whether anything was fetched.
    ///
    /// The current generation of a key is the file at the key path, and
    /// is stale when its marker names another checksum. An older
    /// generation is a file under `.blobs`, named by its checksum.
    pub async fn ensure_bytes(&self, obj: &Object) -> ApiResult<bool> {
        let bucket = &obj.spec.bucket;
        let sha = &obj.status.sha256;
        if !obj.status.latest {
            let Some(path) = blob_file(&self.root, bucket, sha) else {
                return Ok(false);
            };
            if tokio::fs::try_exists(&path).await.map_err(io_err)? {
                return Ok(false);
            }
            let Some(peers) = &self.peers else {
                return Ok(false);
            };
            peers
                .fetch(bucket, sha, obj.status.size_bytes, &path)
                .await?;
            return Ok(true);
        }
        let path = self.key_path(bucket, &obj.spec.key)?;
        let held = tokio::fs::try_exists(&path).await.map_err(io_err)?;
        let marker = crate::replica::read_marker(&self.root, bucket, &obj.spec.key).await;
        if held && marker.is_none_or(|m| &m.sha256 == sha) {
            return Ok(false);
        }
        let Some(peers) = &self.peers else {
            return Ok(false);
        };
        peers
            .fetch(bucket, sha, obj.status.size_bytes, &path)
            .await?;
        crate::replica::write_marker(&self.root, bucket, &obj.spec.key, sha)
            .await
            .map_err(io_err)?;
        Ok(true)
    }

    /// Open bytes this server holds for an object of `bucket_id` with
    /// checksum `sha256`, and their length. Reads this disk only; it is
    /// what a peer is served, so it never asks a peer itself.
    pub async fn open_blob(
        &self,
        bucket_id: &Id,
        sha256: &str,
    ) -> ApiResult<(tokio::fs::File, u64)> {
        let missing = || ApiError::not_found("no such bytes on this server");
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let mut paths = Vec::new();
        let mut size = None;
        for obj in self.objects_for_bucket(&bucket)? {
            if obj.status.sha256 != sha256 {
                continue;
            }
            size = Some(obj.status.size_bytes);
            if obj.status.latest {
                let marker =
                    crate::replica::read_marker(&self.root, &bucket.meta.id, &obj.spec.key).await;
                // A file marked with another checksum is not these bytes.
                if marker.is_none_or(|m| m.sha256 == sha256) {
                    paths.push(self.key_path(&bucket.meta.id, &obj.spec.key)?);
                }
            }
        }
        let size = size.ok_or_else(missing)?;
        paths.extend(blob_file(&self.root, &bucket.meta.id, sha256));
        for path in paths {
            let Ok(file) = tokio::fs::File::open(&path).await else {
                continue;
            };
            if file.metadata().await.map_err(io_err)?.len() == size {
                return Ok((file, size));
            }
        }
        Err(missing())
    }

    /// Object metadata only (no body). Used by HEAD.
    pub fn head_object(&self, bucket_id: &Id, key: &str) -> ApiResult<Object> {
        validate_key(key)?;
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        self.find_current(&bucket, key)?
            .ok_or_else(|| ApiError::not_found(format!("object '{key}' not found")))
    }

    pub async fn delete_object(&self, bucket_id: &Id, key: &str) -> ApiResult<Object> {
        validate_key(key)?;
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let all = self.objects_for_bucket(&bucket)?;
        let doomed: Vec<Object> = all.iter().filter(|o| o.spec.key == key).cloned().collect();
        let current = pick_current(doomed.clone())
            .ok_or_else(|| ApiError::not_found(format!("object '{key}' not found")))?;
        let keep: BTreeSet<String> = all
            .iter()
            .filter(|o| o.spec.key != key)
            .map(|o| o.status.sha256.clone())
            .collect();
        for obj in &doomed {
            self.store.collection::<Object>().delete(&obj.meta.id)?;
        }
        remove_file(&self.key_path(&bucket.meta.id, key)?).await?;
        crate::replica::remove_marker(&self.root, &bucket.meta.id, key).await;
        for obj in &doomed {
            if !keep.contains(&obj.status.sha256) {
                if let Some(blob) = blob_file(&self.root, &bucket.meta.id, &obj.status.sha256) {
                    remove_file(&blob).await?;
                }
            }
        }
        Ok(current)
    }

    /// Latest generation of each key in the bucket, ordered by key.
    pub fn list_objects(&self, bucket_id: &Id) -> ApiResult<Vec<Object>> {
        self.list_objects_filtered(bucket_id, None, None, usize::MAX, None)
            .map(|(items, _)| {
                items
                    .into_iter()
                    .filter_map(|e| match e {
                        ListObjectEntry::Object(o) => Some(*o),
                        ListObjectEntry::CommonPrefix(_) => None,
                    })
                    .collect()
            })
    }

    /// Latest generation of each key, optionally filtered by `prefix` and
    /// starting after `after_key` (exclusive), capped at `limit`.
    ///
    /// Non-empty `delimiter` rolls keys into [`ListObjectEntry::CommonPrefix`]
    /// (each counts once toward `limit`, and toward ListObjectsV2 KeyCount).
    /// When `after_key` is a common-prefix marker (ends with the delimiter),
    /// every key under that prefix is skipped so pagination does not
    /// re-emit the same CommonPrefix.
    /// Returns `(page, truncated)`.
    pub fn list_objects_filtered(
        &self,
        bucket_id: &Id,
        prefix: Option<&str>,
        after_key: Option<&str>,
        limit: usize,
        delimiter: Option<&str>,
    ) -> ApiResult<(Vec<ListObjectEntry>, bool)> {
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let all = self.objects_for_bucket(&bucket)?;
        let prefix = prefix.unwrap_or("");
        let delimiter = delimiter.filter(|d| !d.is_empty());
        let mut keys: Vec<String> = all.iter().map(|o| o.spec.key.clone()).collect();
        keys.sort();
        keys.dedup();
        if !prefix.is_empty() {
            keys.retain(|k| k.starts_with(prefix));
        }
        if let Some(after) = after_key.filter(|a| !a.is_empty()) {
            keys.retain(|k| crate::delimiter::key_after_marker(k, after, delimiter));
        }
        let objects: Vec<Object> = keys
            .into_iter()
            .filter_map(|key| {
                let matches = all.iter().filter(|o| o.spec.key == key).cloned().collect();
                pick_current(matches)
            })
            .collect();
        let entries: Vec<ListObjectEntry> = match delimiter {
            Some(d) => crate::delimiter::collapse_with_delimiter(objects, prefix, d, |o| {
                o.spec.key.as_str()
            })
            .into_iter()
            .map(|e| match e {
                crate::delimiter::Delimited::Item(o) => ListObjectEntry::Object(Box::new(o)),
                crate::delimiter::Delimited::CommonPrefix(cp) => ListObjectEntry::CommonPrefix(cp),
            })
            .collect(),
            None => objects
                .into_iter()
                .map(|o| ListObjectEntry::Object(Box::new(o)))
                .collect(),
        };
        let truncated = entries.len() > limit;
        let page = entries.into_iter().take(limit).collect();
        Ok((page, truncated))
    }

    #[allow(clippy::too_many_arguments)]
    fn insert_object(
        &self,
        bucket: &Bucket,
        key: &str,
        content_type: String,
        sha: String,
        md5: String,
        size: u64,
        version: u64,
    ) -> ApiResult<Object> {
        let spec = ObjectSpec {
            bucket: bucket.meta.id.clone(),
            key: key.to_owned(),
            content_type,
        };
        let mut obj = Object::new("object", bucket.meta.project.clone(), spec);
        obj.meta.name = obj.meta.id.to_string();
        obj.status = ObjectStatus {
            sha256: sha,
            md5,
            size_bytes: size,
            version,
            latest: true,
        };
        self.store
            .collection::<Object>()
            .create(obj)
            .map_err(Into::into)
    }

    fn bucket_dir(&self, id: &Id) -> PathBuf {
        self.root.join(id.to_string())
    }

    fn objects_for_bucket(&self, bucket: &Bucket) -> ApiResult<Vec<Object>> {
        let items = self
            .store
            .collection::<Object>()
            .list(bucket.meta.project.as_ref())?;
        Ok(items
            .into_iter()
            .filter(|o| o.spec.bucket == bucket.meta.id)
            .collect())
    }

    fn find_current(&self, bucket: &Bucket, key: &str) -> ApiResult<Option<Object>> {
        let matches = self
            .objects_for_bucket(bucket)?
            .into_iter()
            .filter(|o| o.spec.key == key)
            .collect();
        Ok(pick_current(matches))
    }

    async fn preserve_previous(&self, bucket: &Id, sha: &str, src: &Path) -> ApiResult<()> {
        let Some(dest) = blob_file(&self.root, bucket, sha) else {
            return Ok(());
        };
        if tokio::fs::try_exists(&dest).await.map_err(io_err)? {
            return Ok(());
        }
        if !tokio::fs::try_exists(src).await.map_err(io_err)? {
            return Ok(());
        }
        if let Some(parent) = dest.parent() {
            tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
        }
        tokio::fs::copy(src, &dest).await.map_err(io_err)?;
        Ok(())
    }
}

/// One row in a ListObjectsV2 page (object or rolled-up prefix).
#[derive(Clone, Debug, PartialEq)]
pub enum ListObjectEntry {
    Object(Box<Object>),
    CommonPrefix(String),
}

impl ListObjectEntry {
    pub fn sort_key(&self) -> &str {
        match self {
            Self::Object(o) => o.spec.key.as_str(),
            Self::CommonPrefix(p) => p.as_str(),
        }
    }
}

fn pick_current(mut matches: Vec<Object>) -> Option<Object> {
    matches.sort_by(|a, b| {
        b.status
            .latest
            .cmp(&a.status.latest)
            .then(b.status.version.cmp(&a.status.version))
            .then(b.meta.revision.cmp(&a.meta.revision))
    });
    matches.into_iter().next()
}

fn blob_file(root: &Path, bucket: &Id, sha: &str) -> Option<PathBuf> {
    if sha.len() != 64 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some(
        root.join(bucket.to_string())
            .join(".blobs")
            .join(sha.to_ascii_lowercase()),
    )
}

fn clean_name(name: &str) -> ApiResult<String> {
    let name = name.trim();
    if name.is_empty() || name.len() > 128 || name.contains('/') {
        return Err(ApiError::bad_request(
            "name must be 1-128 characters without '/'",
        ));
    }
    Ok(name.to_owned())
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(Sha256::digest(bytes).as_slice())
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

fn io_err(e: std::io::Error) -> ApiError {
    ApiError::internal(format!("silo: {e}"))
}

async fn remove_file(path: &Path) -> ApiResult<()> {
    match tokio::fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(io_err(e)),
    }
}

struct Written {
    sha256: String,
    md5: String,
    size: u64,
}

/// Preserve [`ApiError`] stream errors (including `details.s3_code`); wrap others.
pub(crate) fn map_stream_err<S, E>(body: S) -> impl Stream<Item = Result<Bytes, ApiError>> + Send
where
    S: Stream<Item = Result<Bytes, E>> + Send,
    E: IntoBodyReadErr + Send,
{
    use futures::StreamExt;
    body.map(|item| item.map_err(IntoBodyReadErr::into_body_read_err))
}

pub trait IntoBodyReadErr {
    fn into_body_read_err(self) -> ApiError;
}

impl IntoBodyReadErr for ApiError {
    fn into_body_read_err(self) -> ApiError {
        self
    }
}

impl IntoBodyReadErr for std::io::Error {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

impl IntoBodyReadErr for String {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

impl IntoBodyReadErr for &str {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

impl IntoBodyReadErr for axum::Error {
    fn into_body_read_err(self) -> ApiError {
        ApiError::bad_request(format!("reading body: {self}"))
    }
}

/// Best-effort cleanup of a staging path if the future is cancelled or errors
/// before rename. Call [`TmpGuard::disarm`] after a successful rename so Drop
/// does not attempt another delete of the (now gone) temp path.
pub(crate) struct TmpGuard(Option<std::path::PathBuf>);

impl TmpGuard {
    pub(crate) fn new(path: std::path::PathBuf) -> Self {
        Self(Some(path))
    }

    /// Stop Drop from deleting the path (use after a successful rename).
    pub(crate) fn disarm(&mut self) {
        self.0 = None;
    }
}

impl Drop for TmpGuard {
    fn drop(&mut self) {
        if let Some(path) = self.0.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn write_stream_atomic<S>(
    dest: &Path,
    body: S,
    max: usize,
    content_length: Option<u64>,
    expected_sha256: Option<&str>,
) -> ApiResult<Written>
where
    S: Stream<Item = Result<Bytes, ApiError>> + Send,
{
    use futures::StreamExt;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
    }
    let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    let mut tmp_guard = TmpGuard::new(tmp.clone());
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(io_err)?;
        let mut sha = Sha256::new();
        let mut md5 = Md5::new();
        let mut size: u64 = 0;
        let mut body = std::pin::pin!(body);
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
            let next = size.saturating_add(chunk.len() as u64);
            if next > max as u64 {
                return Err(ApiError::bad_request(object_too_large_message(max)));
            }
            if let Some(len) = content_length {
                if next > len {
                    return Err(ApiError::bad_request(
                        "body longer than Content-Length".to_owned(),
                    ));
                }
            }
            sha.update(&chunk);
            md5.update(&chunk);
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
                return Err(crate::multipart::s3_err(
                    "XAmzContentSHA256Mismatch",
                    "The provided 'x-amz-content-sha256' header does not match what was computed.",
                ));
            }
        }
        tokio::fs::rename(&tmp, dest).await.map_err(io_err)?;
        Ok(Written {
            sha256,
            md5: hex_encode(md5.finalize().as_slice()),
            size,
        })
    }
    .await;
    if result.is_ok() {
        tmp_guard.disarm();
    } else {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}

#[cfg(test)]
mod tmp_guard_tests {
    use super::TmpGuard;

    #[test]
    fn disarm_skips_delete() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("keep-me");
        std::fs::write(&path, b"hi").unwrap();
        let mut g = TmpGuard::new(path.clone());
        g.disarm();
        drop(g);
        assert!(path.exists(), "disarmed guard must not delete the path");
    }

    #[test]
    fn drop_deletes_when_armed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("remove-me");
        std::fs::write(&path, b"hi").unwrap();
        drop(TmpGuard::new(path.clone()));
        assert!(!path.exists(), "armed guard must delete on drop");
    }
}
