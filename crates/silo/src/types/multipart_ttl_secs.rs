// SPDX-License-Identifier: Apache-2.0
// Copyright (c) 2026 P. Industries LLC and PiStack contributors

/// Default age after which an unfinished multipart upload is removed
/// (7 days).
pub const MULTIPART_TTL_SECS: u64 = 7 * 24 * 60 * 60;
