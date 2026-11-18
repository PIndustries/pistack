// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use super::detected_format::DetectedFormat;
use super::inspection::Inspection;
use super::qemu_info::QemuInfo;

/// Parse `qemu-img info --output=json`.
pub fn parse_qemu_img_info(json: &str) -> ApiResult<Inspection> {
    let info: QemuInfo = serde_json::from_str(json)
        .map_err(|e| ApiError::internal(format!("qemu-img output: {e}")))?;
    let format = match info.format.as_deref() {
        Some("qcow2") => DetectedFormat::Qcow2,
        Some("raw") | None => DetectedFormat::Raw,
        Some(other) => {
            return Err(ApiError::unprocessable(format!(
                "image format '{other}' is not supported; use raw or qcow2"
            )))
        }
    };
    let backing_file = info
        .backing_filename
        .filter(|s| !s.is_empty())
        .or(info.full_backing_filename.filter(|s| !s.is_empty()));
    let data_file = info
        .format_specific
        .and_then(|f| f.data)
        .and_then(|d| d.data_file)
        .filter(|s| !s.is_empty());
    Ok(Inspection {
        format,
        virtual_size: info.virtual_size,
        backing_file,
        data_file,
        source: "qemu-img",
    })
}
