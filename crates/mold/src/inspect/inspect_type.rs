// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::path::Path;

use crate::store::ImageData;
use crate::types::ImageFormat;

use super::detect_magic::detect_magic;
use super::detected_format::DetectedFormat;
use super::inspection::Inspection;
use super::qemu_img_available::qemu_img_available;
use super::read_head::read_head;
use super::run_qemu_img::run_qemu_img;

/// Inspect a stored blob: `qemu-img` when present and the blob is a local
/// file, else the header read from `data`.
pub async fn inspect(
    local_path: Option<&Path>,
    data: Option<ImageData>,
    declared: ImageFormat,
) -> ApiResult<Inspection> {
    if let (Some(p), true) = (local_path, qemu_img_available()) {
        let mut insp = run_qemu_img(p, declared).await?;
        // qemu was told the format, so confirm with the magic bytes too.
        if let Ok(mut f) = tokio::fs::File::open(p).await {
            let mut head = [0u8; 512];
            let n = read_head(&mut f, &mut head).await;
            let by_magic = detect_magic(&head[..n]);
            if by_magic.format == DetectedFormat::Qcow2 && insp.format == DetectedFormat::Raw {
                insp.format = DetectedFormat::Qcow2;
            }
        }
        return Ok(insp);
    }
    let mut data: ImageData =
        match (data, local_path) {
            (Some(d), _) => d,
            (None, Some(p)) => Box::pin(tokio::fs::File::open(p).await.map_err(|e| {
                ApiError::internal(format!("inspect: opening {}: {e}", p.display()))
            })?),
            (None, None) => return Err(ApiError::internal("inspect: no data to read")),
        };
    let mut head = vec![0u8; 4096];
    let n = read_head(&mut data, &mut head).await;
    Ok(detect_magic(&head[..n]))
}
