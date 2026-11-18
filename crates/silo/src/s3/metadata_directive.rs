// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

use crate::multipart::{self};
use axum::http::HeaderMap;
use pistack_types::ApiResult;

use super::header_str::header_str;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MetadataDirective {
    Copy,
    Replace,
}

pub(crate) fn metadata_directive(headers: &HeaderMap) -> ApiResult<MetadataDirective> {
    match header_str(headers, "x-amz-metadata-directive") {
        None => Ok(MetadataDirective::Copy),
        Some(raw) => match raw.to_ascii_uppercase().as_str() {
            "COPY" => Ok(MetadataDirective::Copy),
            "REPLACE" => Ok(MetadataDirective::Replace),
            _ => Err(multipart::s3_err(
                "InvalidArgument",
                "x-amz-metadata-directive must be COPY or REPLACE",
            )),
        },
    }
}
