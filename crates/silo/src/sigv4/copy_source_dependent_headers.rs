// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Headers that must be signed when present alongside `x-amz-copy-source`.
/// Shared by UploadPartCopy and CopyObject.
pub(crate) const COPY_SOURCE_DEPENDENT_HEADERS: &[&str] = &[
    "x-amz-copy-source-range",
    "x-amz-copy-source-if-match",
    "x-amz-copy-source-if-none-match",
    "x-amz-copy-source-if-modified-since",
    "x-amz-copy-source-if-unmodified-since",
    "x-amz-metadata-directive",
];
