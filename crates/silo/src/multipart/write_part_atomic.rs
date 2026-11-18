// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::types::*;
use bytes::Bytes;
use futures::Stream;
use md5::Md5;
use pistack_types::{ApiError, ApiResult};
use sha2::{Digest, Sha256};
use std::path::Path;
use subtle::ConstantTimeEq;
use tokio::io::AsyncWriteExt;

use super::hex_encode::hex_encode;
use super::io_err::io_err;
use super::part_written::PartWritten;
use super::s3_err::s3_err;

pub(crate) async fn write_part_atomic<S>(
    dest: &Path,
    body: S,
    max: usize,
    content_length: Option<u64>,
    expected_sha256: Option<&str>,
) -> ApiResult<PartWritten>
where
    S: Stream<Item = Result<Bytes, ApiError>> + Send,
{
    use futures::StreamExt;

    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_err)?;
    }
    let tmp = dest.with_file_name(format!(".tmp-{}", uuid::Uuid::now_v7()));
    let mut tmp_guard = crate::client::TmpGuard::new(tmp.clone());
    let result = async {
        let mut file = tokio::fs::File::create(&tmp).await.map_err(io_err)?;
        let mut md5 = Md5::new();
        let mut sha = Sha256::new();
        let mut size: u64 = 0;
        let mut body = std::pin::pin!(body);
        while let Some(chunk) = body.next().await {
            let chunk = chunk?;
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
    if result.is_ok() {
        tmp_guard.disarm();
    } else {
        // Discard the temp file only. A prior successful part at `dest`
        // stays until a later UploadPart renames over it.
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    result
}
