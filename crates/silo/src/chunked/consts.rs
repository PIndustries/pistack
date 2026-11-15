// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Hard cap on one aws-chunked data chunk so verify-before-write stays bounded.
pub const MAX_CHUNK_BYTES: usize = 16 * 1024 * 1024;
/// Cap on a single chunk-header line (`size;chunk-signature=…`) before CRLF.
pub const MAX_CHUNK_HEADER_BYTES: usize = 128;
/// Cap on the entire trailing-headers section (including final blank line).
pub const MAX_TRAILER_BYTES: usize = 4 * 1024;
/// Cap on trailing header lines (excluding the blank terminator).
pub const MAX_TRAILER_LINES: usize = 8;

pub const STREAMING_PAYLOAD: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD";
pub const STREAMING_UNSIGNED_TRAILER: &str = "STREAMING-UNSIGNED-PAYLOAD-TRAILER";
pub const STREAMING_SIGNED_TRAILER: &str = "STREAMING-AWS4-HMAC-SHA256-PAYLOAD-TRAILER";
