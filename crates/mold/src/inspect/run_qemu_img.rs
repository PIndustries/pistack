// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};
use std::path::Path;

use crate::types::ImageFormat;

use super::inspection::Inspection;
use super::parse_qemu_img_info::parse_qemu_img_info;

pub(crate) async fn run_qemu_img(path: &Path, declared: ImageFormat) -> ApiResult<Inspection> {
    let out = tokio::process::Command::new("qemu-img")
        .arg("info")
        .arg("--output=json")
        .arg("-f")
        .arg(declared.to_string())
        .arg(path)
        .output()
        .await
        .map_err(|e| ApiError::internal(format!("running qemu-img: {e}")))?;
    if !out.status.success() {
        return Err(ApiError::unprocessable(format!(
            "qemu-img could not read the image as {declared}: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        )));
    }
    parse_qemu_img_info(&String::from_utf8_lossy(&out.stdout))
}
