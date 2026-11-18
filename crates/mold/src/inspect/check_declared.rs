// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use pistack_types::{ApiError, ApiResult};

use crate::types::ImageFormat;

use super::detected_format::DetectedFormat;
use super::inspection::Inspection;

/// Compare what was declared with what was found.
pub fn check_declared(declared: ImageFormat, found: &Inspection) -> ApiResult<()> {
    match (declared, found.format) {
        (ImageFormat::Qcow2, DetectedFormat::Qcow2) | (ImageFormat::Raw, DetectedFormat::Raw) => {
            Ok(())
        }
        (ImageFormat::Qcow2, DetectedFormat::Raw) => Err(ApiError::unprocessable(
            "image declared qcow2 but has no qcow2 header",
        )),
        (ImageFormat::Raw, DetectedFormat::Qcow2) => Err(ApiError::unprocessable(
            "image declared raw but carries a qcow2 header; declare it qcow2 or convert it",
        )),
        (ImageFormat::Oci, _) => Ok(()),
    }
}
