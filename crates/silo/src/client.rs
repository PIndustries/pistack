// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

//! Bucket and object writes.
//!
//! Bytes for the current generation of a key live at
//! `{root}/{bucket-id}/{key}`, where `root` is `$PISTACK_DATA_DIR/silo` in
//! the module. `status.sha256` is the content address of those bytes.
//! With bucket versioning, the previous generation's bytes are kept at
//! `{root}/{bucket-id}/.blobs/{sha256}` and the old record stays with
//! `status.latest = false`.

use crate::types::*;
use pistack_store::Store;
use pistack_types::{ApiError, ApiResult, Id, Meta, Phase, Spec};
use sha2::{Digest, Sha256};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use tokio::io::AsyncWriteExt;

#[derive(Clone)]
pub struct Client {
    store: Store,
    root: PathBuf,
}

impl Client {
    pub fn new(store: Store, root: impl Into<PathBuf>) -> Self {
        Self {
            store,
            root: root.into(),
        }
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
        self.store
            .collection::<Bucket>()
            .create(bucket)
            .map_err(Into::into)
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
        self.store
            .collection::<Bucket>()
            .delete(id)
            .map_err(Into::into)
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

    pub async fn put_object(
        &self,
        bucket_id: &Id,
        key: &str,
        bytes: &[u8],
        content_type: Option<String>,
    ) -> ApiResult<Object> {
        if bytes.len() > MAX_OBJECT_BYTES {
            return Err(ApiError::bad_request("object is larger than 1 GiB"));
        }
        validate_key(key)?;
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let path = self.key_path(&bucket.meta.id, key)?;
        let sha = sha256_hex(bytes);
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
        write_atomic(&path, bytes).await?;

        if bucket.spec.versioning {
            if let Some(prev) = existing {
                let mut old = prev;
                old.status.latest = false;
                self.store.collection::<Object>().update(old, None)?;
            }
            return self.insert_object(&bucket, key, ctype, sha, bytes.len() as u64, version);
        }

        if let Some(prev) = existing {
            let mut next = prev;
            next.spec.content_type = ctype;
            next.status.sha256 = sha;
            next.status.size_bytes = bytes.len() as u64;
            next.status.version = version;
            next.status.latest = true;
            return self
                .store
                .collection::<Object>()
                .update(next, None)
                .map_err(Into::into);
        }
        self.insert_object(&bucket, key, ctype, sha, bytes.len() as u64, version)
    }

    pub async fn get_object(&self, bucket_id: &Id, key: &str) -> ApiResult<(Object, Vec<u8>)> {
        validate_key(key)?;
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let obj = self
            .find_current(&bucket, key)?
            .ok_or_else(|| ApiError::not_found(format!("object '{key}' not found")))?;
        let path = self.key_path(&bucket.meta.id, key)?;
        let bytes = match tokio::fs::read(&path).await {
            Ok(b) => b,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Err(ApiError::not_found(format!("object '{key}' not found")));
            }
            Err(e) => return Err(io_err(e)),
        };
        let got = sha256_hex(&bytes);
        if got != obj.status.sha256 {
            return Err(ApiError::internal(format!(
                "object '{key}' sha256 mismatch: status {}, disk {got}",
                obj.status.sha256
            )));
        }
        Ok((obj, bytes))
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
        let bucket = self.store.collection::<Bucket>().get(bucket_id)?;
        let all = self.objects_for_bucket(&bucket)?;
        let mut keys: Vec<String> = all.iter().map(|o| o.spec.key.clone()).collect();
        keys.sort();
        keys.dedup();
        let mut out = Vec::new();
        for key in keys {
            let matches = all.iter().filter(|o| o.spec.key == key).cloned().collect();
            if let Some(cur) = pick_current(matches) {
                out.push(cur);
            }
        }
        Ok(out)
    }

    fn insert_object(
        &self,
        bucket: &Bucket,
        key: &str,
        content_type: String,
        sha: String,
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

async fn write_atomic(dest: &Path, bytes: &[u8]) -> ApiResult<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
    }
    let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(io_err)?;
        file.write_all(bytes).await.map_err(io_err)?;
        file.flush().await.map_err(io_err)?;
        file.sync_all().await.map_err(io_err)?;
        drop(file);
        tokio::fs::rename(&tmp, dest).await.map_err(io_err)?;
        Ok(())
    }
    .await;
    if result.is_err() {
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}
